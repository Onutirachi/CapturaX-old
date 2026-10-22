//! Sinalização WebRTC do CapturaX.
//!
//! Só apresenta os dois lados: quem transmite (o app, "host") e quem assiste (o navegador,
//! "viewer") trocam por aqui ofertas/respostas SDP e candidatos ICE, alguns KB por pessoa. O vídeo
//! vai direto entre eles (P2P); nada de mídia passa por este servidor.
//!
//! - Host: `GET /signal?role=host&key=<segredo>&pass=<senha>&nick=<apelido>`. A senha é a de uso
//!   ou a de admin do servidor (`passwords.rs`): só transmite por aqui quem o dono deixou. A sala é
//!   o nome do link (`names.rs`): o apelido, ou um código gerado, preso ao `sha256(segredo)`, então
//!   quem só conhece o link não consegue se passar pelo host.
//! - Viewer: `GET /signal?role=viewer&room=<nome>`. Se o host não está, espera; quando ele entra,
//!   recebe o aviso e a conexão começa.
//!
//! Mensagens (JSON):
//! - para o host: `ready {room, role}`, `unauthorized`, `name_taken`, `name_invalid`, `join {peer}`,
//!   `leave {peer}`, `signal {peer, data}`
//! - do host: `signal {peer, data}`, `kick {peer}`
//! - para o viewer: `waiting`, `live`, `ended`, `signal {data}`, `kicked`
//! - do viewer: `signal {data}`
//!
//! API (JSON, para o CapturaX): `POST /api/role {password}` → `{role: "admin"|"usage"|null}`;
//! `POST /api/usage-password {admin, password}` troca a senha de uso (vazia = só o admin);
//! `POST /api/claim {password, key, nickname}` reserva o nome do link → `{name}` (409 = em uso).
//! O resto do site é a página de quem assiste (`CAPTURAX_SITE`).
//!
//! `capturax-signal reset-admin` gera uma senha de admin nova (perdeu a antiga).

mod names;
mod passwords;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use names::{valid_name, ClaimError, Names};
use passwords::{Passwords, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};
use tower_http::services::{ServeDir, ServeFile};

/// Mensagem maior que isso é recusada (um SDP tem poucos KB).
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_VIEWERS_PER_ROOM: usize = 100;
const MAX_ROOMS: usize = 2000;
const MAX_CONNECTIONS_PER_IP: usize = 30;
/// Mensagens por segundo que um viewer pode mandar (candidatos ICE chegam em rajada).
const VIEWER_MESSAGES_PER_SEC: u32 = 60;
const PING_EVERY: Duration = Duration::from_secs(20);
/// Fila de saída de cada conexão; se encher (cliente parado), ela é derrubada.
const OUTBOX: usize = 256;
/// Senhas erradas por IP por minuto (depois disso, espera).
const MAX_FAILURES_PER_MINUTE: u32 = 10;

type Outbox = mpsc::Sender<Utf8Bytes>;

#[derive(Default)]
struct Room {
    /// (id da conexão, fila) do host conectado agora.
    host: Option<(u64, Outbox)>,
    viewers: HashMap<u64, Outbox>,
}

struct Hub {
    rooms: Mutex<HashMap<String, Room>>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    next_id: AtomicU64,
    passwords: Passwords,
    names: Names,
    /// Muda quando a senha de uso muda: quem transmite com ela é desconectado.
    usage_generation: watch::Sender<u64>,
    failures: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl Hub {
    /// IP que errou a senha muitas vezes no último minuto.
    fn locked_out(&self, ip: IpAddr) -> bool {
        let failures = self.failures.lock().unwrap();
        failures.get(&ip).is_some_and(|(since, count)| since.elapsed() < Duration::from_secs(60) && *count >= MAX_FAILURES_PER_MINUTE)
    }

    fn failed(&self, ip: IpAddr) {
        let mut failures = self.failures.lock().unwrap();
        let entry = failures.entry(ip).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= Duration::from_secs(60) {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
    }

    /// Confere a senha fora do executor (o hash é propositalmente lento).
    async fn role(self: &Arc<Self>, ip: IpAddr, password: String) -> Option<Role> {
        if self.locked_out(ip) {
            return None;
        }
        let hub = self.clone();
        let role = tokio::task::spawn_blocking(move || hub.passwords.role(&password)).await.ok().flatten();
        if role.is_none() {
            self.failed(ip);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        role
    }
}

#[derive(Deserialize)]
struct Params {
    role: String,
    key: Option<String>,
    room: Option<String>,
    pass: Option<String>,
    nick: Option<String>,
}

/// Dono de um segredo (o app): 40 caracteres hex do SHA-256. Os nomes de link pertencem a ele.
fn owner_of(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

fn text(value: Value) -> Utf8Bytes {
    Utf8Bytes::from(value.to_string())
}

/// IP de quem conectou: o nginx (local) informa o real em `X-Real-IP`.
fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if peer.ip().is_loopback() {
        if let Some(ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()) {
            return ip;
        }
    }
    peer.ip()
}

/// Libera a vaga do IP quando a conexão termina.
struct IpSlot {
    hub: Arc<Hub>,
    ip: IpAddr,
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut per_ip = self.hub.per_ip.lock().unwrap();
        if let Some(count) = per_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                per_ip.remove(&self.ip);
            }
        }
    }
}

async fn signal(
    ws: WebSocketUpgrade,
    Query(params): Query<Params>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(hub): State<Arc<Hub>>,
) -> Response {
    let ip = client_ip(&headers, peer);
    {
        let mut per_ip = hub.per_ip.lock().unwrap();
        let count = per_ip.entry(ip).or_default();
        if *count >= MAX_CONNECTIONS_PER_IP {
            return (StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response();
        }
        *count += 1;
    }
    let slot = IpSlot { hub: hub.clone(), ip };
    let ws = ws.max_message_size(MAX_MESSAGE_BYTES).max_frame_size(MAX_MESSAGE_BYTES);
    match (params.role.as_str(), params.key, params.room) {
        ("host", Some(key), _) if (16..=256).contains(&key.len()) => {
            let role = hub.role(ip, params.pass.unwrap_or_default()).await;
            // Só quem pode transmitir reserva nome.
            let room = match role {
                Some(_) => claim(&hub, owner_of(&key), params.nick).await,
                None => Err(ClaimError::Invalid),
            };
            ws.on_upgrade(move |socket| host(socket, hub, room, role, slot))
        }
        ("viewer", _, Some(room)) if valid_name(&room) => ws.on_upgrade(move |socket| viewer(socket, hub, room, slot)),
        _ => (StatusCode::BAD_REQUEST, "bad request").into_response(),
    }
}

/// Liga a fila de saída ao socket e devolve o receptor das mensagens que chegam.
fn split(socket: WebSocket) -> (Outbox, futures_util::stream::SplitStream<WebSocket>, tokio::task::JoinHandle<()>) {
    let (mut sink, stream) = socket.split();
    let (outbox, mut queue) = mpsc::channel::<Utf8Bytes>(OUTBOX);
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                message = queue.recv() => match message {
                    Some(message) => if sink.send(Message::Text(message)).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Default::default())).await.is_err() { break },
            }
        }
        let _ = sink.close().await;
    });
    (outbox, stream, writer)
}

/// Mensagem de texto JSON recebida (pings/pongs e binários são ignorados; fechar encerra).
async fn next_json(stream: &mut futures_util::stream::SplitStream<WebSocket>) -> Option<Option<Value>> {
    match stream.next().await? {
        Ok(Message::Text(text)) => Some(serde_json::from_str(text.as_str()).ok()),
        Ok(Message::Close(_)) | Err(_) => None,
        Ok(_) => Some(None),
    }
}

/// Reserva o nome do link fora do executor (lê/grava o arquivo).
async fn claim(hub: &Arc<Hub>, owner: String, nickname: Option<String>) -> Result<String, ClaimError> {
    let hub = hub.clone();
    tokio::task::spawn_blocking(move || hub.names.claim(&owner, nickname.as_deref())).await.unwrap_or(Err(ClaimError::Io))
}

async fn host(socket: WebSocket, hub: Arc<Hub>, room: Result<String, ClaimError>, role: Option<Role>, _slot: IpSlot) {
    let id = hub.next_id.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut stream, writer) = split(socket);
    // Recusas avisadas pelo próprio WebSocket (o navegador não lê o status HTTP).
    let refusal = match (&role, &room) {
        (None, _) => Some("unauthorized"),
        (_, Err(ClaimError::Taken)) => Some("name_taken"),
        (_, Err(_)) => Some("name_invalid"),
        _ => None,
    };
    if let Some(refusal) = refusal {
        let _ = outbox.send(text(json!({ "type": refusal }))).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        writer.abort();
        return;
    }
    let (Some(role), Ok(room)) = (role, room) else { return };
    let mut usage_changes = hub.usage_generation.subscribe();
    let waiting: Vec<(u64, Outbox)> = {
        let mut rooms = hub.rooms.lock().unwrap();
        if !rooms.contains_key(&room) && rooms.len() >= MAX_ROOMS {
            return;
        }
        let entry = rooms.entry(room.clone()).or_default();
        // Host reconectando (ou o app aberto de novo): o novo assume a sala.
        entry.host = Some((id, outbox.clone()));
        entry.viewers.iter().map(|(peer, outbox)| (*peer, outbox.clone())).collect()
    };
    let _ = outbox.try_send(text(json!({ "type": "ready", "room": room, "role": role.name() })));
    for (peer, viewer) in waiting {
        let _ = viewer.try_send(text(json!({ "type": "live" })));
        let _ = outbox.try_send(text(json!({ "type": "join", "peer": peer })));
    }

    loop {
        let message = tokio::select! {
            message = next_json(&mut stream) => message,
            // Senha de uso trocada: quem entrou com a antiga sai.
            Ok(()) = usage_changes.changed(), if role == Role::Usage => {
                let _ = outbox.send(text(json!({ "type": "unauthorized" }))).await;
                tokio::time::sleep(Duration::from_millis(200)).await;
                None
            }
        };
        let Some(message) = message else { break };
        let Some(message) = message else { continue };
        let peer = message.get("peer").and_then(Value::as_u64);
        let target = peer.and_then(|peer| hub.rooms.lock().unwrap().get(&room).and_then(|r| r.viewers.get(&peer).cloned()));
        match (message.get("type").and_then(Value::as_str), target) {
            (Some("signal"), Some(viewer)) => {
                let data = message.get("data").cloned().unwrap_or(Value::Null);
                if viewer.try_send(text(json!({ "type": "signal", "data": data }))).is_err() {
                    // Viewer parado: a fila encheu; a conexão dele cai sozinha.
                }
            }
            (Some("kick"), Some(viewer)) => {
                let _ = viewer.try_send(text(json!({ "type": "kicked" })));
                if let Some(room) = hub.rooms.lock().unwrap().get_mut(&room) {
                    room.viewers.remove(&peer.unwrap());
                }
            }
            _ => {}
        }
    }

    // Saiu: avisa quem assiste (se ainda for o host atual) e limpa a sala vazia.
    {
        let mut rooms = hub.rooms.lock().unwrap();
        if let Some(entry) = rooms.get_mut(&room) {
            if entry.host.as_ref().is_some_and(|(host, _)| *host == id) {
                entry.host = None;
                for viewer in entry.viewers.values() {
                    let _ = viewer.try_send(text(json!({ "type": "ended" })));
                }
            }
            if entry.host.is_none() && entry.viewers.is_empty() {
                rooms.remove(&room);
            }
        }
    }
    writer.abort();
    // Transmitiu agora: o nome vale por mais 7 dias.
    let names_hub = hub.clone();
    let name = room.clone();
    let _ = tokio::task::spawn_blocking(move || names_hub.names.touch(&name)).await;
}

async fn viewer(socket: WebSocket, hub: Arc<Hub>, room: String, _slot: IpSlot) {
    let id = hub.next_id.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut stream, writer) = split(socket);
    let host = {
        let mut rooms = hub.rooms.lock().unwrap();
        if !rooms.contains_key(&room) && rooms.len() >= MAX_ROOMS {
            return;
        }
        let entry = rooms.entry(room.clone()).or_default();
        if entry.viewers.len() >= MAX_VIEWERS_PER_ROOM {
            let _ = outbox.try_send(text(json!({ "type": "full" })));
            None
        } else {
            entry.viewers.insert(id, outbox.clone());
            Some(entry.host.as_ref().map(|(_, host)| host.clone()))
        }
    };
    let Some(host) = host else {
        tokio::time::sleep(Duration::from_millis(200)).await;
        writer.abort();
        return;
    };
    match host {
        Some(host) => {
            let _ = outbox.try_send(text(json!({ "type": "live" })));
            let _ = host.try_send(text(json!({ "type": "join", "peer": id })));
        }
        None => {
            let _ = outbox.try_send(text(json!({ "type": "waiting" })));
        }
    }

    let mut window = Instant::now();
    let mut count = 0u32;
    while let Some(message) = next_json(&mut stream).await {
        if window.elapsed() >= Duration::from_secs(1) {
            window = Instant::now();
            count = 0;
        }
        count += 1;
        if count > VIEWER_MESSAGES_PER_SEC {
            break;
        }
        let Some(message) = message else { continue };
        if message.get("type").and_then(Value::as_str) != Some("signal") {
            continue;
        }
        let host = hub.rooms.lock().unwrap().get(&room).and_then(|r| r.host.as_ref().map(|(_, host)| host.clone()));
        if let Some(host) = host {
            let data = message.get("data").cloned().unwrap_or(Value::Null);
            let _ = host.try_send(text(json!({ "type": "signal", "peer": id, "data": data })));
        }
    }

    let mut rooms = hub.rooms.lock().unwrap();
    if let Some(entry) = rooms.get_mut(&room) {
        entry.viewers.remove(&id);
        if let Some((_, host)) = &entry.host {
            let _ = host.try_send(text(json!({ "type": "leave", "peer": id })));
        }
        if entry.host.is_none() && entry.viewers.is_empty() {
            rooms.remove(&room);
        }
    }
    drop(rooms);
    writer.abort();
}

#[derive(Deserialize)]
struct RoleRequest {
    password: String,
}

/// Que senha é esta? O CapturaX usa para validar e para mostrar o campo de admin.
async fn api_role(State(hub): State<Arc<Hub>>, headers: HeaderMap, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(request): Json<RoleRequest>) -> Response {
    let ip = client_ip(&headers, peer);
    if hub.locked_out(ip) {
        return cors((StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "too_many_attempts" }))).into_response());
    }
    let role = hub.role(ip, request.password).await;
    cors(Json(json!({ "role": role.map(Role::name), "usageSet": hub.passwords.usage_is_set() })).into_response())
}

#[derive(Deserialize)]
struct UsageRequest {
    admin: String,
    password: String,
}

/// Troca a senha de uso (só com a de admin). Corta quem transmite com a antiga.
async fn api_usage_password(State(hub): State<Arc<Hub>>, headers: HeaderMap, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(request): Json<UsageRequest>) -> Response {
    let ip = client_ip(&headers, peer);
    if hub.role(ip, request.admin).await != Some(Role::Admin) {
        return cors((StatusCode::UNAUTHORIZED, Json(json!({ "error": "not_admin" }))).into_response());
    }
    if request.password.len() > 200 {
        return cors((StatusCode::BAD_REQUEST, Json(json!({ "error": "too_long" }))).into_response());
    }
    let saved = {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.passwords.set_usage(&request.password)).await
    };
    match saved {
        Ok(Ok(())) => {
            hub.usage_generation.send_modify(|generation| *generation += 1);
            cors(Json(json!({ "ok": true })).into_response())
        }
        _ => cors((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "save_failed" }))).into_response()),
    }
}

#[derive(Deserialize)]
struct ClaimRequest {
    password: String,
    key: String,
    nickname: Option<String>,
}

/// Reserva o nome do link (apelido ou código gerado) para o app mostrar antes de transmitir.
async fn api_claim(State(hub): State<Arc<Hub>>, headers: HeaderMap, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(request): Json<ClaimRequest>) -> Response {
    let ip = client_ip(&headers, peer);
    if !(16..=256).contains(&request.key.len()) {
        return cors((StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_key" }))).into_response());
    }
    if hub.role(ip, request.password).await.is_none() {
        return cors((StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response());
    }
    match claim(&hub, owner_of(&request.key), request.nickname).await {
        Ok(name) => cors(Json(json!({ "name": name })).into_response()),
        Err(ClaimError::Taken) => cors((StatusCode::CONFLICT, Json(json!({ "error": "taken" }))).into_response()),
        Err(ClaimError::Invalid) => cors((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))).into_response()),
        Err(ClaimError::Io) => cors((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "save_failed" }))).into_response()),
    }
}

/// A API é chamada pelo app (outra origem): libera CORS. A segurança é a senha, não a origem.
fn cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("POST, OPTIONS"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type"));
    headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    response
}

async fn preflight(method: Method) -> Response {
    cors(if method == Method::OPTIONS { StatusCode::NO_CONTENT.into_response() } else { StatusCode::METHOD_NOT_ALLOWED.into_response() })
}

async fn health(State(hub): State<Arc<Hub>>) -> String {
    let rooms = hub.rooms.lock().unwrap();
    let live = rooms.values().filter(|r| r.host.is_some()).count();
    let viewers: usize = rooms.values().map(|r| r.viewers.len()).sum();
    format!("ok rooms={} live={live} viewers={viewers}\n", rooms.len())
}

#[tokio::main]
async fn main() {
    let data_dir = std::path::PathBuf::from(std::env::var("CAPTURAX_DATA").unwrap_or_else(|_| "/data".into()));
    let passwords = Passwords::new(&data_dir);

    // `capturax-signal reset-admin`: senha de admin nova, mostrada no terminal.
    if std::env::args().nth(1).as_deref() == Some("reset-admin") {
        match passwords.reset_admin() {
            Ok(password) => println!("Nova senha de admin / new admin password: {password}"),
            Err(error) => {
                eprintln!("Could not save the password: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    match passwords.ensure_admin(std::env::var("CAPTURAX_ADMIN_PASSWORD").ok()) {
        Ok(Some(password)) => {
            println!("==================================================================");
            println!(" Senha de admin do CapturaX / CapturaX admin password: {password}");
            println!(" Coloque no CapturaX (Configurações → Transmissão). Para gerar outra:");
            println!("   docker compose exec capturax-signal capturax-signal reset-admin");
            println!("==================================================================");
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Could not create the admin password in {}: {error}", data_dir.display());
            std::process::exit(1);
        }
    }

    let address: SocketAddr = std::env::var("CAPTURAX_SIGNAL_ADDR").ok().and_then(|a| a.parse().ok()).unwrap_or(([127, 0, 0, 1], 8790).into());
    let hub = Arc::new(Hub {
        rooms: Mutex::default(),
        per_ip: Mutex::default(),
        next_id: AtomicU64::default(),
        names: Names::new(&data_dir),
        passwords,
        usage_generation: watch::channel(0).0,
        failures: Mutex::default(),
    });
    // Página de quem assiste: qualquer caminho desconhecido (ex.: `/v/<sala>`) recebe o index.html.
    let site = std::path::PathBuf::from(std::env::var("CAPTURAX_SITE").unwrap_or_else(|_| "/site".into()));
    let pages = ServeDir::new(&site).fallback(ServeFile::new(site.join("index.html")));
    let app = Router::new()
        .route("/signal", get(signal))
        .route("/signal/health", get(health))
        .route("/api/role", post(api_role).options(preflight))
        .route("/api/usage-password", post(api_usage_password).options(preflight))
        .route("/api/claim", post(api_claim).options(preflight))
        .fallback_service(pages)
        .with_state(hub);
    let listener = tokio::net::TcpListener::bind(address).await.expect("bind");
    eprintln!("capturax-signal listening on {address}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("serve");
}

#[cfg(test)]
mod tests {
    #[test]
    fn owner_is_derived_from_key() {
        let owner = super::owner_of("segredo-de-teste-123");
        assert_eq!(owner.len(), 40);
        assert_ne!(owner, super::owner_of("outro-segredo-123"));
    }
}
