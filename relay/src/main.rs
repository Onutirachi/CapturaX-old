//! Sinalização WebRTC do CapturaX.
//!
//! Só apresenta os dois lados: quem transmite (o app, "host") e quem assiste (o navegador,
//! "viewer") trocam por aqui ofertas/respostas SDP e candidatos ICE, alguns KB por pessoa. O vídeo
//! vai direto entre eles (P2P); nada de mídia passa por este servidor.
//!
//! - Host: `GET /signal?role=host&key=<segredo>`. A sala é `sha256(segredo)` (40 hex), então quem só
//!   conhece o link (que tem a sala) não consegue se passar pelo host.
//! - Viewer: `GET /signal?role=viewer&room=<sala>`. Se o host não está, espera; quando ele entra,
//!   recebe o aviso e a conexão começa.
//!
//! Mensagens (JSON):
//! - para o host: `ready {room}`, `join {peer}`, `leave {peer}`, `signal {peer, data}`
//! - do host: `signal {peer, data}`, `kick {peer}`
//! - para o viewer: `waiting`, `live`, `ended`, `signal {data}`, `kicked`
//! - do viewer: `signal {data}`

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

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

type Outbox = mpsc::Sender<Utf8Bytes>;

#[derive(Default)]
struct Room {
    /// (id da conexão, fila) do host conectado agora.
    host: Option<(u64, Outbox)>,
    viewers: HashMap<u64, Outbox>,
}

#[derive(Default)]
struct Hub {
    rooms: Mutex<HashMap<String, Room>>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    next_id: AtomicU64,
}

#[derive(Deserialize)]
struct Params {
    role: String,
    key: Option<String>,
    room: Option<String>,
}

/// Sala de um segredo: 40 caracteres hex do SHA-256 (curto para o link, longo para não adivinhar).
fn room_of(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

fn valid_room(room: &str) -> bool {
    room.len() == 40 && room.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
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
        ("host", Some(key), _) if (16..=256).contains(&key.len()) => ws.on_upgrade(move |socket| host(socket, hub, room_of(&key), slot)),
        ("viewer", _, Some(room)) if valid_room(&room) => ws.on_upgrade(move |socket| viewer(socket, hub, room, slot)),
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

async fn host(socket: WebSocket, hub: Arc<Hub>, room: String, _slot: IpSlot) {
    let id = hub.next_id.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut stream, writer) = split(socket);
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
    let _ = outbox.try_send(text(json!({ "type": "ready", "room": room })));
    for (peer, viewer) in waiting {
        let _ = viewer.try_send(text(json!({ "type": "live" })));
        let _ = outbox.try_send(text(json!({ "type": "join", "peer": peer })));
    }

    while let Some(message) = next_json(&mut stream).await {
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
    drop(rooms);
    writer.abort();
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

async fn health(State(hub): State<Arc<Hub>>) -> String {
    let rooms = hub.rooms.lock().unwrap();
    let live = rooms.values().filter(|r| r.host.is_some()).count();
    let viewers: usize = rooms.values().map(|r| r.viewers.len()).sum();
    format!("ok rooms={} live={live} viewers={viewers}\n", rooms.len())
}

#[tokio::main]
async fn main() {
    let address: SocketAddr = std::env::var("CAPTURAX_SIGNAL_ADDR").ok().and_then(|a| a.parse().ok()).unwrap_or(([127, 0, 0, 1], 8790).into());
    let hub = Arc::new(Hub::default());
    let app = Router::new().route("/signal", get(signal)).route("/signal/health", get(health)).with_state(hub);
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
    fn room_is_derived_from_key() {
        let room = super::room_of("segredo-de-teste-123");
        assert_eq!(room.len(), 40);
        assert!(super::valid_room(&room));
        assert_ne!(room, super::room_of("outro-segredo-123"));
    }
}
