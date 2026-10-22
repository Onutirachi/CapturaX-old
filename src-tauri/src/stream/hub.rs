//! Guarda a transmissão em memória e quem está assistindo.
//!
//! Dois jeitos de assistir, com os mesmos pedaços de fMP4 (~100 ms cada):
//! - **Baixa latência** (`live.mp4`): uma conexão HTTP que fica aberta; quem chega recebe o
//!   cabeçalho, os pedaços desde o último keyframe e depois cada pedaço novo assim que sai.
//! - **HLS** (Safari/iPhone, sem MediaSource): os pedaços de keyframe a keyframe viram segmentos.

use axum::body::Bytes;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

use crate::clock;
use crate::media::fmp4::{AudioTrack, Fragment, VideoTrack};
use crate::media::FragmentSink;

/// Segmentos HLS mantidos (≈ 10 s). O playlist lista os mais recentes.
const KEEP_SEGMENTS: usize = 10;
const PLAYLIST_SEGMENTS: usize = 6;
/// Pedaços na fila de cada espectador antes de ele ser considerado atrasado demais (≈ 25 s).
const LIVE_QUEUE_PARTS: usize = 256;
/// Proteção caso o encoder pare de mandar keyframes.
const MAX_GOP_PARTS: usize = 300;
/// Sem pedir nada por esse tempo, o espectador conta como saído.
const VIEWER_TIMEOUT: Duration = Duration::from_secs(15);

struct Segment {
    sequence: u64,
    duration: f64,
    data: Bytes,
}

#[derive(Default)]
struct Live {
    /// Muda a cada transmissão; vai na URL para o player não misturar duas transmissões.
    session: u64,
    init: Option<Bytes>,
    codecs: String,
    /// Pedaços desde o último keyframe: é por onde um espectador novo começa.
    gop: Vec<Bytes>,
    sender: Option<broadcast::Sender<Bytes>>,
    segments: VecDeque<Segment>,
    building: Vec<u8>,
    building_duration: f64,
    next_sequence: u64,
}

struct Viewer {
    first_seen: Instant,
    last_seen: Instant,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ViewerInfo {
    pub ip: String,
    pub watching_secs: u64,
}

/// Começo de uma conexão de baixa latência.
pub struct LiveStart {
    pub init: Bytes,
    pub backlog: Vec<Bytes>,
    pub receiver: broadcast::Receiver<Bytes>,
}

#[derive(Default)]
pub struct StreamHub {
    live: Mutex<Live>,
    viewers: Mutex<HashMap<IpAddr, Viewer>>,
    blocked: RwLock<HashSet<IpAddr>>,
    /// Pessoas assistindo ao mesmo tempo; 0 = sem limite.
    max_viewers: std::sync::atomic::AtomicUsize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub live: bool,
    pub session: String,
    pub codecs: String,
}

impl StreamHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn begin(&self) {
        let (sender, _) = broadcast::channel(LIVE_QUEUE_PARTS);
        *self.live.lock().unwrap() = Live { session: clock::now_us() as u64, sender: Some(sender), ..Default::default() };
    }

    /// Fecha as conexões abertas (o canal acaba) e volta para "Offline".
    fn end(&self) {
        *self.live.lock().unwrap() = Live::default();
    }

    pub fn status(&self) -> LiveStatus {
        let live = self.live.lock().unwrap();
        LiveStatus { live: live.init.is_some() && !live.gop.is_empty(), session: live.session.to_string(), codecs: live.codecs.clone() }
    }

    pub fn is_live(&self) -> bool {
        self.status().live
    }

    pub fn start_live(&self, session: u64) -> Option<LiveStart> {
        // Tudo sob o mesmo lock: nenhum pedaço se perde nem se repete entre o atraso e a fila.
        let live = self.live.lock().unwrap();
        if live.session != session || live.gop.is_empty() {
            return None;
        }
        Some(LiveStart { init: live.init.clone()?, backlog: live.gop.clone(), receiver: live.sender.as_ref()?.subscribe() })
    }

    pub fn playlist(&self, session: u64) -> Option<String> {
        let live = self.live.lock().unwrap();
        if live.session != session || live.init.is_none() || live.segments.is_empty() {
            return None;
        }
        let skip = live.segments.len().saturating_sub(PLAYLIST_SEGMENTS);
        let window: Vec<&Segment> = live.segments.iter().skip(skip).collect();
        let target = window.iter().map(|s| s.duration.ceil() as u64).max().unwrap_or(1).max(2);
        let mut text = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"init.mp4\"\n",
            window[0].sequence
        );
        for segment in window {
            text.push_str(&format!("#EXTINF:{:.3},\n{}.m4s\n", segment.duration, segment.sequence));
        }
        Some(text)
    }

    pub fn init(&self, session: u64) -> Option<Bytes> {
        let live = self.live.lock().unwrap();
        (live.session == session).then(|| live.init.clone()).flatten()
    }

    pub fn segment(&self, session: u64, sequence: u64) -> Option<Bytes> {
        let live = self.live.lock().unwrap();
        if live.session != session {
            return None;
        }
        live.segments.iter().find(|s| s.sequence == sequence).map(|s| s.data.clone())
    }

    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        self.blocked.read().unwrap().contains(&ip)
    }

    pub fn set_blocked(&self, ips: &[String]) {
        let parsed = ips.iter().filter_map(|ip| ip.parse().ok()).collect();
        *self.blocked.write().unwrap() = parsed;
        let blocked = self.blocked.read().unwrap();
        self.viewers.lock().unwrap().retain(|ip, _| !blocked.contains(ip));
    }

    /// Registra o espectador; `false` se o IP estiver bloqueado. Sem limite de espectadores: quem
    /// transmite conhece a própria internet (o servidor ainda limita conexões por IP e no total).
    pub fn touch_viewer(&self, ip: IpAddr) -> bool {
        if self.is_blocked(ip) {
            return false;
        }
        let now = Instant::now();
        let mut viewers = self.viewers.lock().unwrap();
        viewers.retain(|_, v| now.duration_since(v.last_seen) < VIEWER_TIMEOUT);
        if let Some(viewer) = viewers.get_mut(&ip) {
            viewer.last_seen = now;
            return true;
        }
        viewers.insert(ip, Viewer { first_seen: now, last_seen: now });
        true
    }

    pub fn set_max_viewers(&self, max: u32) {
        self.max_viewers.store(max as usize, std::sync::atomic::Ordering::Relaxed);
    }

    /// Alguém novo (que ainda não está assistindo) passaria do limite.
    pub fn is_full_for(&self, ip: IpAddr) -> bool {
        let max = self.max_viewers.load(std::sync::atomic::Ordering::Relaxed);
        if max == 0 {
            return false;
        }
        let now = Instant::now();
        let viewers = self.viewers.lock().unwrap();
        let active = viewers.values().filter(|v| now.duration_since(v.last_seen) < VIEWER_TIMEOUT).count();
        !viewers.contains_key(&ip) && active >= max
    }

    pub fn viewers(&self) -> Vec<ViewerInfo> {
        let now = Instant::now();
        let mut viewers = self.viewers.lock().unwrap();
        viewers.retain(|_, v| now.duration_since(v.last_seen) < VIEWER_TIMEOUT);
        viewers.iter().map(|(ip, v)| ViewerInfo { ip: ip.to_string(), watching_secs: now.duration_since(v.first_seen).as_secs() }).collect()
    }
}

/// Destino da sessão de transmissão: os fragmentos vão para a memória do hub.
pub struct LiveSink {
    hub: Arc<StreamHub>,
}

impl LiveSink {
    pub fn new(hub: Arc<StreamHub>) -> Self {
        hub.begin();
        Self { hub }
    }
}

impl FragmentSink for LiveSink {
    fn init(&mut self, bytes: &[u8], _moov_offset: usize, codecs: &str) -> std::io::Result<()> {
        let mut live = self.hub.live.lock().unwrap();
        live.init = Some(Bytes::copy_from_slice(bytes));
        live.codecs = codecs.to_string();
        Ok(())
    }

    fn fragment(&mut self, fragment: &Fragment) -> std::io::Result<()> {
        let part = Bytes::copy_from_slice(&fragment.bytes);
        let mut live = self.hub.live.lock().unwrap();
        if fragment.starts_with_key {
            // Fecha o segmento HLS anterior (de keyframe a keyframe).
            if !live.building.is_empty() {
                let sequence = live.next_sequence;
                live.next_sequence += 1;
                let data = Bytes::from(std::mem::take(&mut live.building));
                let duration = std::mem::take(&mut live.building_duration);
                live.segments.push_back(Segment { sequence, duration, data });
                while live.segments.len() > KEEP_SEGMENTS {
                    live.segments.pop_front();
                }
            }
            live.gop.clear();
        }
        if live.gop.is_empty() && !fragment.starts_with_key {
            // Ainda sem keyframe: ninguém conseguiria começar a tocar daqui.
            return Ok(());
        }
        live.building.extend_from_slice(&part);
        live.building_duration += fragment.duration_secs;
        if live.gop.len() < MAX_GOP_PARTS {
            live.gop.push(part.clone());
        }
        if let Some(sender) = &live.sender {
            let _ = sender.send(part);
        }
        Ok(())
    }

    fn finish(&mut self, _video: &VideoTrack, _audio: &[AudioTrack]) -> std::io::Result<Option<PathBuf>> {
        self.hub.end();
        Ok(None)
    }
}

impl Drop for LiveSink {
    fn drop(&mut self) {
        self.hub.end();
    }
}
