//! Servidor da transmissão por link (HLS), rodando no próprio PC.
//!
//! Superfície mínima de propósito: só GET em rotas fixas, tudo servido da memória (nada é lido do
//! disco a partir da URL), nenhuma rota controla o app, e há limites de conexões, de tamanho de
//! cabeçalho e de tempo. O vídeo é público para quem tiver o link; o PC não fica exposto.

mod ipv6;
pub mod firewall;
mod public_ip;
mod hub;
pub mod upnp;

pub use hub::{LiveSink, StreamHub, ViewerInfo};

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Request, Response, StatusCode};
use axum::routing::get;
use axum::{Extension, Router};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tower::ServiceExt;
use upnp::UpnpStatus;

/// Página de quem assiste (gerada por `npm run build:viewer`).
#[derive(RustEmbed)]
#[folder = "viewer-dist/"]
#[allow_missing = true]
struct ViewerAssets;

const MAX_CONNECTIONS: usize = 128;
const MAX_CONNECTIONS_PER_IP: usize = 12;
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy)]
struct ClientIp(IpAddr);

#[derive(Default)]
struct Connections {
    total: AtomicUsize,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
}

/// Libera a vaga da conexão quando ela termina.
struct ConnectionSlot {
    connections: Arc<Connections>,
    ip: IpAddr,
}

impl Connections {
    fn acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnectionSlot> {
        let mut per_ip = self.per_ip.lock().unwrap();
        let count = per_ip.entry(ip).or_default();
        if *count >= MAX_CONNECTIONS_PER_IP || self.total.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            return None;
        }
        *count += 1;
        self.total.fetch_add(1, Ordering::Relaxed);
        Some(ConnectionSlot { connections: self.clone(), ip })
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut per_ip = self.connections.per_ip.lock().unwrap();
        if let Some(count) = per_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                per_ip.remove(&self.ip);
            }
        }
        self.connections.total.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running: bool,
    pub port: u16,
    pub local_url: Option<String>,
    pub public_url: Option<String>,
    /// Link pelo IPv6 público do PC (sem NAT: dispensa abrir porta; quem assiste precisa de IPv6).
    pub public_url_v6: Option<String>,
    pub upnp: UpnpStatus,
    /// IP público desta conexão (consulta DNS), mesmo sem UPnP.
    pub public_ip: Option<String>,
    /// Atrás de CGNAT: o IP que o roteador vê não é o público, então porta aberta não adianta.
    pub cgnat: bool,
    pub firewall_ok: bool,
    pub live: bool,
    pub viewers: Vec<ViewerInfo>,
    pub error: Option<String>,
}

struct Running {
    port: u16,
    upnp: bool,
    shutdown: watch::Sender<bool>,
    task: tauri::async_runtime::JoinHandle<()>,
}

pub struct StreamServer {
    pub hub: Arc<StreamHub>,
    running: tokio::sync::Mutex<Option<Running>>,
    upnp_status: Arc<Mutex<UpnpStatus>>,
    firewall_ok: AtomicBool,
    error: Mutex<Option<String>>,
    /// O servidor também escuta em IPv6 (para o link IPv6).
    listening_v6: AtomicBool,
    /// Último IP público consultado e quando.
    public_ip: Arc<Mutex<Option<(IpAddr, std::time::Instant)>>>,
    public_ip_pending: Arc<AtomicBool>,
}

const PUBLIC_IP_MAX_AGE: Duration = Duration::from_secs(300);

impl StreamServer {
    pub fn new() -> Self {
        Self {
            hub: StreamHub::new(),
            running: tokio::sync::Mutex::new(None),
            upnp_status: Arc::new(Mutex::new(UpnpStatus::Disabled)),
            firewall_ok: AtomicBool::new(false),
            error: Mutex::new(None),
            listening_v6: AtomicBool::new(false),
            public_ip: Arc::default(),
            public_ip_pending: Arc::default(),
        }
    }

    pub fn refresh_firewall(&self) {
        self.firewall_ok.store(firewall::rule_exists(), Ordering::Relaxed);
    }

    /// Deixa o servidor no estado pedido: liga, desliga, ou reinicia se porta/UPnP mudaram.
    pub async fn apply(&self, enabled: bool, port: u16, upnp: bool) {
        let mut running = self.running.lock().await;
        if let Some(current) = running.as_ref() {
            if enabled && current.port == port && current.upnp == upnp {
                return;
            }
        }
        if let Some(current) = running.take() {
            let _ = current.shutdown.send(true);
            // Não espera para sempre: o UPnP pode demorar a responder (roteador lento). A tarefa
            // termina sozinha em segundo plano; o "Parar" da transmissão não pode travar nisso.
            let _ = tokio::time::timeout(SHUTDOWN_WAIT, current.task).await;
        }
        *self.upnp_status.lock().unwrap() = UpnpStatus::Disabled;
        *self.error.lock().unwrap() = None;
        if !enabled {
            return;
        }

        let listener = match TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await {
            Ok(listener) => listener,
            Err(error) => {
                *self.error.lock().unwrap() = Some(crate::error::coded("port_unavailable", serde_json::json!({ "port": port, "detail": error.to_string() })));
                return;
            }
        };
        // IPv6 ajuda quem está atrás de CGNAT no IPv4; se não der, segue só com IPv4.
        let listener_v6 = TcpListener::bind(SocketAddr::from(([0u16; 8], port))).await.ok();
        self.listening_v6.store(listener_v6.is_some(), Ordering::Relaxed);
        self.refresh_firewall();

        let (shutdown, shutdown_rx) = watch::channel(false);
        let hub = self.hub.clone();
        let upnp_status = self.upnp_status.clone();
        let task = tauri::async_runtime::spawn(async move {
            let router = router(hub.clone());
            let connections = Arc::new(Connections::default());
            let mut accepts = Vec::new();
            for listener in std::iter::once(listener).chain(listener_v6) {
                accepts.push(tokio::spawn(accept_loop(listener, router.clone(), hub.clone(), connections.clone(), shutdown_rx.clone())));
            }
            let mapping = if upnp { Some(tokio::spawn(manage_upnp(port, upnp_status, shutdown_rx.clone()))) } else { None };
            for accept in accepts {
                let _ = accept.await;
            }
            if let Some(mapping) = mapping {
                let _ = mapping.await;
            }
        });
        *running = Some(Running { port, upnp, shutdown, task });
    }

    /// Consulta o IP público em segundo plano se o valor guardado estiver velho.
    fn refresh_public_ip(&self) {
        let fresh = self.public_ip.lock().unwrap().is_some_and(|(_, at)| at.elapsed() < PUBLIC_IP_MAX_AGE);
        if fresh || self.public_ip_pending.swap(true, Ordering::Relaxed) {
            return;
        }
        let (cache, pending) = (self.public_ip.clone(), self.public_ip_pending.clone());
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(ip) = public_ip::lookup() {
                *cache.lock().unwrap() = Some((IpAddr::V4(ip), std::time::Instant::now()));
            }
            pending.store(false, Ordering::Relaxed);
        });
    }

    pub async fn status(&self) -> ServerStatus {
        let running = self.running.lock().await;
        let port = running.as_ref().map_or(0, |r| r.port);
        let upnp = self.upnp_status.lock().unwrap().clone();
        let live = self.hub.is_live();
        if running.is_some() {
            self.refresh_public_ip();
        }
        let public_ip = self.public_ip.lock().unwrap().map(|(ip, _)| ip);
        // CGNAT: o roteador diz ter um IP que não é o público (ou um IP de faixa da operadora).
        let cgnat = match (&upnp, public_ip) {
            (UpnpStatus::Mapped { cgnat: true, .. }, _) => true,
            (UpnpStatus::Mapped { external_ip, .. }, Some(public)) => external_ip.parse::<IpAddr>().ok() != Some(public),
            _ => false,
        };
        // IP público pela consulta DNS; se ela ainda não respondeu, o que o roteador informou.
        let mapped_ip = match &upnp {
            UpnpStatus::Mapped { external_ip, .. } => external_ip.parse::<IpAddr>().ok(),
            _ => None,
        };
        let public_url = public_ip.or(mapped_ip).filter(|_| !cgnat).map(|ip| format!("http://{ip}:{port}/"));
        ServerStatus {
            running: running.is_some(),
            port,
            local_url: running.as_ref().and_then(|_| lan_ip()).map(|ip| format!("http://{ip}:{port}/")),
            public_url,
            public_url_v6: running
                .as_ref()
                .filter(|_| self.listening_v6.load(Ordering::Relaxed))
                .and_then(|_| ipv6::public_ipv6())
                .map(|ip| format!("http://[{ip}]:{port}/")),
            upnp,
            public_ip: public_ip.map(|ip| ip.to_string()),
            cgnat,
            firewall_ok: self.firewall_ok.load(Ordering::Relaxed),
            live,
            viewers: self.hub.viewers(),
            error: self.error.lock().unwrap().clone(),
        }
    }
}

/// IP desta máquina na rede local. O `connect` de UDP só escolhe a rota; nenhum pacote sai.
fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// Quanto o desligamento do servidor espera as tarefas (UPnP) terminarem.
const SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

async fn manage_upnp(port: u16, status: Arc<Mutex<UpnpStatus>>, mut shutdown: watch::Receiver<bool>) {
    *status.lock().unwrap() = UpnpStatus::Searching;
    // A busca pelo roteador pode levar segundos: desligar no meio dela não espera o fim.
    let mapped = tokio::select! {
        mapped = upnp::map(port) => mapped,
        _ = shutdown.changed() => return,
    };
    let mapping = match mapped {
        Ok((mapping, mapped)) => {
            *status.lock().unwrap() = mapped;
            mapping
        }
        Err(reason) => {
            *status.lock().unwrap() = UpnpStatus::Failed { reason };
            return;
        }
    };
    loop {
        tokio::select! {
            _ = tokio::time::sleep(upnp::RENEW_EVERY) => {
                if let Err(error) = mapping.renew().await {
                    eprintln!("UPnP renewal failed: {error}");
                }
            }
            _ = shutdown.changed() => break,
        }
    }
    // Tirar a porta do roteador também não pode segurar o desligamento.
    let _ = tokio::time::timeout(SHUTDOWN_WAIT, mapping.remove()).await;
}

async fn accept_loop(listener: TcpListener, router: Router, hub: Arc<StreamHub>, connections: Arc<Connections>, mut shutdown: watch::Receiver<bool>) {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let (stream, address) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(_) => continue,
            },
            _ = shutdown.changed() => break,
        };
        let ip = canonical_ip(address.ip());
        if hub.is_blocked(ip) {
            continue;
        }
        let Some(slot) = connections.acquire(ip) else { continue };
        let router = router.clone();
        tasks.spawn(async move {
            let _slot = slot;
            let _ = stream.set_nodelay(true);
            let service = hyper::service::service_fn(move |mut request: Request<Incoming>| {
                request.extensions_mut().insert(ClientIp(ip));
                router.clone().oneshot(request)
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_TIMEOUT)
                .max_buf_size(MAX_HEADER_BYTES)
                .serve_connection(TokioIo::new(stream), service);
            // Sem limite de duração: a conexão de baixa latência fica aberta enquanto a pessoa assiste.
            // Conexões paradas caem pelo timeout de cabeçalho; o total por IP é limitado acima.
            let _ = connection.await;
        });
        // Limpa tarefas terminadas sem bloquear.
        while tasks.try_join_next().is_some() {}
    }
    tasks.abort_all();
}

/// `::ffff:1.2.3.4` (IPv4 via socket IPv6) vira `1.2.3.4`, para bloqueio e contagem baterem.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        ip => ip,
    }
}

fn router(hub: Arc<StreamHub>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
        .route("/live/status", get(status))
        .route("/live/{session}/index.m3u8", get(playlist))
        .route("/live/{session}/init.mp4", get(init_segment))
        .route("/live/{session}/live.mp4", get(live_stream))
        .route("/live/{session}/{file}", get(media_segment))
        .fallback(|| async { respond(StatusCode::NOT_FOUND, "text/plain", "Not found", false) })
        .with_state(hub)
}

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
                   media-src 'self' blob:; worker-src 'self' blob:; connect-src 'self'; font-src 'self'; \
                   frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn respond(status: StatusCode, content_type: &str, body: impl Into<Body>, cacheable: bool) -> Response<Body> {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert("Cross-Origin-Resource-Policy", HeaderValue::from_static("same-origin"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(if cacheable { "public, max-age=60" } else { "no-store" }));
    if content_type.starts_with("text/html") {
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    }
    response
}

fn embedded(path: &str) -> Response<Body> {
    match ViewerAssets::get(path) {
        Some(file) => {
            let mime = file.metadata.mimetype().to_string();
            // Arquivos em assets/ têm hash no nome; o HTML sempre revalida.
            respond(StatusCode::OK, &mime, file.data.into_owned(), path.starts_with("assets/"))
        }
        None if path == "index.html" => respond(StatusCode::SERVICE_UNAVAILABLE, "text/plain", "Viewer page not built (run npm run build:viewer)", false),
        None => respond(StatusCode::NOT_FOUND, "text/plain", "Not found", false),
    }
}

async fn index() -> Response<Body> {
    embedded("index.html")
}

async fn asset(Path(path): Path<String>) -> Response<Body> {
    embedded(&format!("assets/{path}"))
}

async fn status(State(hub): State<Arc<StreamHub>>) -> Response<Body> {
    let body = serde_json::to_string(&hub.status()).unwrap_or_default();
    respond(StatusCode::OK, "application/json", body, false)
}

/// Baixa latência: cabeçalho + pedaços desde o último keyframe, depois cada pedaço novo assim
/// que sai do encoder, numa resposta que não termina. Quem ficar atrasado demais é desconectado
/// (a página reconecta e volta para o ao vivo).
async fn live_stream(State(hub): State<Arc<StreamHub>>, Extension(ClientIp(ip)): Extension<ClientIp>, Path(session): Path<u64>) -> Response<Body> {
    // Limite de espectadores: a página mostra "cheio" com o 503.
    if hub.is_full_for(ip) {
        return respond(StatusCode::SERVICE_UNAVAILABLE, "text/plain", "Full", false);
    }
    if !hub.touch_viewer(ip) {
        return blocked();
    }
    let Some(start) = hub.start_live(session) else {
        return respond(StatusCode::NOT_FOUND, "text/plain", "Offline", false);
    };
    let first: Vec<Result<Bytes, std::convert::Infallible>> = std::iter::once(start.init).chain(start.backlog).map(Ok).collect();
    let following = futures_util::stream::unfold((start.receiver, hub, ip), |(mut receiver, hub, ip)| async move {
        match receiver.recv().await {
            Ok(part) => {
                hub.touch_viewer(ip);
                Some((Ok(part), (receiver, hub, ip)))
            }
            // Atrasado (rede lenta) ou transmissão encerrada: fecha a resposta.
            Err(_) => None,
        }
    });
    let stream = futures_util::StreamExt::chain(futures_util::stream::iter(first), following);
    respond(StatusCode::OK, "video/mp4", Body::from_stream(stream), false)
}

fn blocked() -> Response<Body> {
    respond(StatusCode::FORBIDDEN, "text/plain", "Forbidden", false)
}

async fn playlist(State(hub): State<Arc<StreamHub>>, Extension(ClientIp(ip)): Extension<ClientIp>, Path(session): Path<u64>) -> Response<Body> {
    if !hub.touch_viewer(ip) {
        return blocked();
    }
    match hub.playlist(session) {
        Some(text) => respond(StatusCode::OK, "application/vnd.apple.mpegurl", text, false),
        None => respond(StatusCode::NOT_FOUND, "text/plain", "Offline", false),
    }
}

async fn init_segment(State(hub): State<Arc<StreamHub>>, Path(session): Path<u64>) -> Response<Body> {
    match hub.init(session) {
        Some(data) => respond(StatusCode::OK, "video/mp4", data, true),
        None => respond(StatusCode::NOT_FOUND, "text/plain", "Offline", false),
    }
}

async fn media_segment(State(hub): State<Arc<StreamHub>>, Extension(ClientIp(ip)): Extension<ClientIp>, Path((session, file)): Path<(u64, String)>) -> Response<Body> {
    let Some(sequence) = file.strip_suffix(".m4s").and_then(|n| n.parse::<u64>().ok()) else {
        return respond(StatusCode::NOT_FOUND, "text/plain", "Not found", false);
    };
    if !hub.touch_viewer(ip) {
        return blocked();
    }
    match hub.segment(session, sequence) {
        Some(data) => respond(StatusCode::OK, "video/iso.segment", data, true),
        None => respond(StatusCode::NOT_FOUND, "text/plain", "Not found", false),
    }
}

#[cfg(test)]
mod tests {
    //! Sobe o servidor com os segmentos de um fixture (pasta `hls/` do teste do muxer), repetindo-os
    //! como uma transmissão ao vivo, para testar de fora (navegador, ffmpeg, curl):
    //! `CAPTURAX_FIXTURE=<pasta> CAPTURAX_SERVE_SECS=30 cargo test --lib serve_fixture -- --ignored`
    use super::*;
    use crate::media::fmp4::Fragment;
    use crate::media::FragmentSink;

    #[test]
    #[ignore]
    fn serve_fixture() {
        let dir = std::path::PathBuf::from(std::env::var("CAPTURAX_FIXTURE").expect("CAPTURAX_FIXTURE")).join("hls");
        let secs: u64 = std::env::var("CAPTURAX_SERVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
        let server = StreamServer::new();
        tauri::async_runtime::block_on(server.apply(true, 18787, false));
        let mut sink = LiveSink::new(server.hub.clone());
        sink.init(&std::fs::read(dir.join("init.mp4")).unwrap(), 0, "avc1.64001f,mp4a.40.2").unwrap();
        let mut segments: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "m4s")).collect();
        segments.sort();
        println!("serving {} segments on http://127.0.0.1:18787/ for {secs}s", segments.len());
        let start = std::time::Instant::now();
        let mut i = 0;
        while start.elapsed().as_secs() < secs {
            let bytes = std::fs::read(&segments[i % segments.len()]).unwrap();
            // Os segmentos do fixture começam todos em keyframe.
            sink.fragment(&Fragment { bytes, payload_offset: 0, video: vec![], audio: vec![], duration_secs: 1.0, starts_with_key: true }).unwrap();
            i += 1;
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
        println!("viewers at the end: {:?}", server.hub.viewers());
    }
}
