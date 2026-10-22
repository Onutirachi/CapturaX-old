//! Comandos chamados pelo front (`invoke`).
//!
//! Os comandos que mexem com áudio, disco ou rede são `async` para rodarem fora da thread
//! principal (a da janela), que no Tauri executa os comandos síncronos.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, State};

use crate::audio::{display_name, AudioEngine, DeviceInfo, MixSettings, PreviewSettings};
use crate::clock;
use crate::config::{Config, ConfigStore};
use crate::media::fmp4::{VideoColor, VideoTrack};
use crate::media::{parse_codec, FileSink, MediaManager, SessionKind, SessionOptions, SessionStatus};
use crate::stream::{firewall, LiveSink, ServerStatus, StreamServer};

pub struct AppState {
    pub config: Mutex<ConfigStore>,
    pub audio: AudioEngine,
    pub media: MediaManager,
    pub stream: StreamServer,
    recording: Mutex<Option<(u32, PathBuf)>>,
    streaming: Mutex<Option<u32>>,
    /// Janela de "Go live" aberta: o servidor fica no ar mesmo sem "Accept viewers".
    preparing: AtomicBool,
    /// Áudio do mesmo aparelho do vídeo do preview (o WebView informa), para o modo automático.
    paired_input: Mutex<Option<String>>,
    /// Saída do mixer que manda o áudio ao WebView para o P2P (WebRTC).
    p2p_audio: Mutex<Option<u64>>,
    /// Áudio do mesmo aparelho da fonte da gravação/transmissão (`None` = usa o do preview).
    output_paired_input: Mutex<Option<String>>,
}

impl AppState {
    pub fn new(config: ConfigStore) -> Self {
        let audio = AudioEngine::new();
        Self {
            config: Mutex::new(config),
            media: MediaManager::new(audio.clone()),
            audio,
            stream: StreamServer::new(),
            recording: Mutex::new(None),
            streaming: Mutex::new(None),
            preparing: AtomicBool::new(false),
            paired_input: Mutex::new(None),
            output_paired_input: Mutex::new(None),
            p2p_audio: Mutex::new(None),
        }
    }

    /// Servidor no ar: aceitando espectadores, ao vivo, ou com a janela de "Go live" aberta.
    fn server_wanted(&self, config: &Config) -> bool {
        config.stream.accept_viewers || self.streaming.lock().unwrap().is_some() || self.preparing.load(Ordering::Relaxed)
    }

    fn config(&self) -> Config {
        self.effective(&self.config.lock().unwrap().current)
    }

    /// Configuração com o áudio automático resolvido (o par do vídeo atual, ou nenhum).
    fn effective(&self, config: &Config) -> Config {
        let mut config = config.clone();
        let paired = self.paired_input.lock().unwrap().clone();
        if config.audio.preview_input_auto {
            config.audio.preview_input = paired.clone();
        }
        // Gravação/transmissão no automático: o áudio do aparelho da fonte; com tela/janela como
        // fonte, o do aparelho do preview (o som do console vai junto).
        // `"none"`: a fonte é um dispositivo sem áudio.
        if config.audio.record_input.is_none() {
            config.audio.record_input = match self.output_paired_input.lock().unwrap().clone() {
                Some(id) if id == "none" => None,
                Some(id) => Some(id),
                None => paired,
            };
        }
        config
    }

    /// Aplica a configuração ao motor de áudio e ao servidor.
    pub async fn apply(&self, config: &Config) {
        let audio = self.audio.clone();
        let effective = self.effective(config);
        let preview = preview_settings(&effective);
        let mix = mix_settings(&effective);
        // Troca de dispositivo espera threads de áudio terminarem: fora do executor async.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            audio.set_preview(preview);
            audio.update_mix(&mix);
        })
        .await;

        let hub = &self.stream.hub;
        hub.set_blocked(&config.stream.blocked_ips);
        hub.set_max_viewers(config.stream.max_viewers);
        self.stream.apply(self.server_wanted(config), config.stream.port, config.stream.upnp).await;
    }

    /// Fecha gravação/transmissão ao sair (a gravação precisa ser finalizada).
    pub fn shutdown(&self) {
        self.media.stop_all();
        self.audio.set_preview(None);
    }
}

fn preview_settings(config: &Config) -> Option<PreviewSettings> {
    let audio = &config.audio;
    audio.native_preview.then(|| PreviewSettings { input: audio.preview_input.clone(), output: audio.preview_output.clone(), delay_ms: audio.preview_delay_ms })
}

fn mix_settings(config: &Config) -> MixSettings {
    let audio = &config.audio;
    let mut inputs = Vec::new();
    if audio.record_input_enabled {
        if let Some(device) = audio.record_input.clone() {
            inputs.push((device, audio.record_input_volume));
        }
    }
    if let Some(mic) = audio.record_mic.clone() {
        inputs.push((mic, audio.record_mic_volume));
    }
    MixSettings {
        inputs,
        mic: audio.record_mic.clone(),
        system: audio.record_system.then_some(audio.record_system_volume),
        excluded: audio.excluded_apps.iter().map(|e| e.to_lowercase()).collect(),
        offset_ms: audio.av_offset_ms,
    }
}

/// Pedaços da transmissão: quanto menor, menor o atraso de quem assiste (e mais pacotes).
const LIVE_PART_SECS: f64 = 0.1;

fn has_audio(settings: &MixSettings) -> bool {
    !settings.inputs.is_empty() || settings.system.is_some()
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string()
}

fn raw_body(request: &Request<'_>) -> Result<Vec<u8>, String> {
    match request.body() {
        InvokeBody::Raw(data) => Ok(data.clone()),
        _ => Err("Expected binary body".into()),
    }
}

fn header<T: std::str::FromStr>(request: &Request<'_>, name: &str) -> Result<T, String> {
    request
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("Missing header {name}"))
}

// ---- Geral ------------------------------------------------------------------------------

/// Relógio QPC em µs; o front usa para alinhar o `performance.now()` com o áudio.
#[tauri::command]
pub fn clock_now() -> i64 {
    clock::now_us()
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    // A salva, sem o áudio automático resolvido (o front devolve isso no `set_config`).
    state.config.lock().unwrap().current.clone()
}

#[tauri::command]
pub async fn set_config(state: State<'_, AppState>, config: Config) -> Result<(), String> {
    {
        let mut store = state.config.lock().unwrap();
        store.current = config.clone();
        store.save();
    }
    state.apply(&config).await;
    Ok(())
}

// ---- Áudio ------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct AudioDevices {
    inputs: Vec<DeviceInfo>,
    outputs: Vec<DeviceInfo>,
}

#[tauri::command]
pub async fn audio_devices(state: State<'_, AppState>) -> Result<AudioDevices, String> {
    Ok(AudioDevices { inputs: state.audio.list_inputs()?, outputs: state.audio.list_outputs()? })
}

#[tauri::command]
pub async fn audio_set_preview_volume(state: State<'_, AppState>, volume: f32, muted: bool) -> Result<(), String> {
    state.audio.set_preview_volume(volume, muted);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppListEntry {
    exe: String,
    name: String,
    running: bool,
    playing: bool,
    excluded: bool,
}

/// Apps para a tela de exclusão: os que têm áudio agora + os já vistos + os excluídos.
#[tauri::command]
pub async fn audio_apps(state: State<'_, AppState>) -> Result<Vec<AppListEntry>, String> {
    let running = state.audio.running_apps()?;
    let mut store = state.config.lock().unwrap();
    let audio = &mut store.current.audio;
    let mut changed = false;
    for app in &running {
        if !audio.known_apps.contains(&app.exe) {
            audio.known_apps.push(app.exe.clone());
            changed = true;
        }
    }
    let mut names: Vec<String> = audio.known_apps.iter().chain(&audio.excluded_apps).map(|e| e.to_lowercase()).collect();
    names.sort();
    names.dedup();
    let mut list: Vec<AppListEntry> = names
        .into_iter()
        .map(|exe| {
            let live = running.iter().find(|a| a.exe == exe);
            AppListEntry {
                name: live.map_or_else(|| display_name(&exe), |a| a.name.clone()),
                running: live.is_some(),
                playing: live.is_some_and(|a| a.playing),
                excluded: audio.excluded_apps.iter().any(|e| e.eq_ignore_ascii_case(&exe)),
                exe,
            }
        })
        .collect();
    list.sort_by(|a, b| (!a.playing, !a.running, a.name.to_lowercase()).cmp(&(!b.playing, !b.running, b.name.to_lowercase())));
    if changed {
        store.save();
    }
    Ok(list)
}

/// Modos reais (resolução × FPS × formato) de cada dispositivo de vídeo.
#[tauri::command]
pub async fn video_modes() -> Result<Vec<crate::video_modes::VideoDeviceModes>, String> {
    let _com = crate::audio::ComGuard::new();
    crate::video_modes::list().map_err(|e| e.to_string())
}

/// O WebView informa o áudio do mesmo aparelho do vídeo atual (`None` = o vídeo não tem áudio).
#[tauri::command]
pub async fn audio_set_paired_input(state: State<'_, AppState>, id: Option<String>) -> Result<(), String> {
    if *state.paired_input.lock().unwrap() == id {
        return Ok(());
    }
    *state.paired_input.lock().unwrap() = id;
    let config = state.config.lock().unwrap().current.clone();
    if config.audio.preview_input_auto || config.audio.record_input.is_none() {
        state.apply(&config).await;
    }
    Ok(())
}

/// Idiomas da interface do Windows, do preferido para o último (ex.: `["pt-BR", "en-US"]`).
#[tauri::command]
pub fn system_languages() -> Vec<String> {
    use windows::core::PWSTR;
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut size) = (0u32, 0u32);
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut size).is_err() {
            return Vec::new();
        }
        let mut buffer = vec![0u16; size as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, Some(PWSTR(buffer.as_mut_ptr())), &mut size).is_err() {
            return Vec::new();
        }
        // Lista de strings separadas por \0, terminada em \0\0.
        buffer.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// O WebView informa o áudio do aparelho da fonte da gravação/transmissão (`None` = tela/janela,
/// usa o do preview; `"none"` = o aparelho não tem áudio).
#[tauri::command]
pub async fn audio_set_output_paired_input(state: State<'_, AppState>, id: Option<String>) -> Result<(), String> {
    if *state.output_paired_input.lock().unwrap() == id {
        return Ok(());
    }
    *state.output_paired_input.lock().unwrap() = id;
    let config = state.config.lock().unwrap().current.clone();
    if config.audio.record_input.is_none() {
        state.apply(&config).await;
    }
    Ok(())
}

// ---- Áudio para o P2P ---------------------------------------------------------------------

/// Começa a mandar a mistura (a mesma da gravação/transmissão) ao WebView, em blocos de 10 ms de
/// PCM 16 bits estéreo 48 kHz (`{"capturax":"pcm","t":µs,"d":base64}`), para virar a trilha de
/// áudio do WebRTC.
#[tauri::command]
pub fn p2p_audio_start(state: State<'_, AppState>, capture: State<'_, crate::capture::ScreenCapture>) {
    use base64::Engine;
    let mut current = state.p2p_audio.lock().unwrap();
    if current.is_some() {
        return;
    }
    let bridge = capture.bridge();
    let mix = mix_settings(&state.config());
    let id = state.audio.add_mix_output(
        &mix,
        Box::new(move |block: &crate::audio::PcmBlock| {
            let pcm: Vec<u8> = block.samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
            let data = base64::engine::general_purpose::STANDARD.encode(pcm);
            bridge.post(format!(r#"{{"capturax":"pcm","t":{},"d":"{data}"}}"#, block.pts_us));
            true
        }),
    );
    *current = Some(id);
}

#[tauri::command]
pub fn p2p_audio_stop(state: State<'_, AppState>) {
    if let Some(id) = state.p2p_audio.lock().unwrap().take() {
        state.audio.remove_mix_output(id);
    }
}

// ---- Captura de tela/janela ---------------------------------------------------------------

#[tauri::command]
pub async fn capture_sources() -> crate::capture::sources::CaptureSources {
    tauri::async_runtime::spawn_blocking(crate::capture::sources::list).await.unwrap_or_default()
}

#[tauri::command]
pub async fn capture_start(capture: State<'_, crate::capture::ScreenCapture>, id: String, fps: u32, cursor: bool) -> Result<crate::capture::CaptureStarted, String> {
    capture.start(&id, fps, cursor)
}

#[tauri::command]
pub fn capture_set_fps(capture: State<'_, crate::capture::ScreenCapture>, fps: u32) {
    capture.set_fps(fps);
}

#[tauri::command]
pub async fn capture_stop(capture: State<'_, crate::capture::ScreenCapture>) -> Result<(), String> {
    capture.stop();
    Ok(())
}

// ---- Gravação e transmissão -------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStarted {
    id: u32,
    path: String,
}

#[tauri::command]
pub async fn recording_start(app: AppHandle, state: State<'_, AppState>) -> Result<RecordingStarted, String> {
    if let Some((id, path)) = state.recording.lock().unwrap().as_ref() {
        return Ok(RecordingStarted { id: *id, path: path.display().to_string() });
    }
    let config = state.config();
    let folder = match &config.recording.folder {
        Some(folder) if !folder.is_empty() => PathBuf::from(folder),
        _ => app.path().video_dir().map_err(|e| e.to_string())?.join("CapturaX"),
    };
    let path = folder.join(format!("CapturaX {}.mp4", timestamp()));
    let sink = FileSink::create(path.clone()).map_err(|e| crate::error::coded("recording_file", serde_json::json!({ "path": path.display().to_string(), "detail": e.to_string() })))?;
    let mix = mix_settings(&config);
    let id = state.media.start(SessionOptions {
        kind: SessionKind::Recording,
        sink: Box::new(sink),
        audio: has_audio(&mix).then_some(mix),
        audio_bitrate_kbps: config.recording.audio_bitrate_kbps,
        separate_mic: config.audio.record_mic_separate && config.audio.record_mic.is_some(),
        fragment_secs: 1.0,
        part_secs: None,
    });
    *state.recording.lock().unwrap() = Some((id, path.clone()));
    Ok(RecordingStarted { id, path: path.display().to_string() })
}

/// Para a gravação e devolve o arquivo final (`None` se nada chegou a ser gravado).
#[tauri::command]
pub async fn recording_stop(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let Some((id, _)) = state.recording.lock().unwrap().take() else { return Ok(None) };
    let media = state.media.clone();
    let result = tauri::async_runtime::spawn_blocking(move || media.stop(id)).await.map_err(|e| e.to_string())??;
    Ok(result.map(|p| p.display().to_string()))
}

#[tauri::command]
pub async fn stream_start(state: State<'_, AppState>) -> Result<u32, String> {
    if let Some(id) = *state.streaming.lock().unwrap() {
        return Ok(id);
    }
    let config = state.config();
    state.stream.apply(true, config.stream.port, config.stream.upnp).await;
    let mix = mix_settings(&config);
    let id = state.media.start(SessionOptions {
        kind: SessionKind::Stream,
        sink: Box::new(LiveSink::new(state.stream.hub.clone())),
        audio: has_audio(&mix).then_some(mix),
        audio_bitrate_kbps: config.stream.audio_bitrate_kbps,
        // Quem assiste ouve uma faixa só.
        separate_mic: false,
        fragment_secs: 1.0,
        part_secs: Some(LIVE_PART_SECS),
    });
    *state.streaming.lock().unwrap() = Some(id);
    state.preparing.store(false, Ordering::Relaxed);
    Ok(id)
}

/// Liga o servidor antes de entrar ao vivo, para o link (e o UPnP) já estar pronto na janela de "Go live".
#[tauri::command]
pub async fn stream_prepare(state: State<'_, AppState>) -> Result<(), String> {
    state.preparing.store(true, Ordering::Relaxed);
    let config = state.config();
    state.stream.apply(true, config.stream.port, config.stream.upnp).await;
    Ok(())
}

/// Desistiu de entrar ao vivo: o servidor volta ao que a configuração pede.
#[tauri::command]
pub async fn stream_unprepare(state: State<'_, AppState>) -> Result<(), String> {
    state.preparing.store(false, Ordering::Relaxed);
    let config = state.config();
    state.stream.apply(state.server_wanted(&config), config.stream.port, config.stream.upnp).await;
    Ok(())
}

#[tauri::command]
pub async fn stream_stop(state: State<'_, AppState>) -> Result<(), String> {
    let Some(id) = state.streaming.lock().unwrap().take() else { return Ok(()) };
    let media = state.media.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || media.stop(id)).await;
    let config = state.config();
    state.stream.apply(state.server_wanted(&config), config.stream.port, config.stream.upnp).await;
    Ok(())
}

#[tauri::command]
pub fn media_video_config(state: State<'_, AppState>, session: u32, codec: String, description: Vec<u8>, width: u32, height: u32, color: Option<VideoColor>) -> Result<(), String> {
    let parsed = parse_codec(&codec).ok_or_else(|| format!("Unsupported codec {codec}"))?;
    state.media.video_config(session, VideoTrack { codec: parsed, description, width, height, codec_string: codec, color });
    Ok(())
}

/// Pacote de vídeo codificado. Corpo binário; sessão, timestamp e keyframe vão nos cabeçalhos.
#[tauri::command]
pub async fn media_video_chunk(state: State<'_, AppState>, request: Request<'_>) -> Result<(), String> {
    let data = raw_body(&request)?;
    let session: u32 = header(&request, "x-session")?;
    let pts_us: i64 = header(&request, "x-pts")?;
    let key = request.headers().get("x-key").is_some_and(|v| v == "1");
    state.media.video_chunk(session, data, pts_us, key);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaStatus {
    sessions: Vec<SessionStatus>,
    recording: Option<RecordingStarted>,
    streaming: Option<u32>,
}

#[tauri::command]
pub async fn media_status(state: State<'_, AppState>) -> Result<MediaStatus, String> {
    let recording = state.recording.lock().unwrap().as_ref().map(|(id, path)| RecordingStarted { id: *id, path: path.display().to_string() });
    Ok(MediaStatus { sessions: state.media.status(), recording, streaming: *state.streaming.lock().unwrap() })
}

#[tauri::command]
pub async fn stream_status(state: State<'_, AppState>) -> Result<ServerStatus, String> {
    Ok(state.stream.status().await)
}

#[tauri::command]
pub async fn firewall_fix(state: State<'_, AppState>) -> Result<(), String> {
    let result = tauri::async_runtime::spawn_blocking(firewall::fix).await.map_err(|e| e.to_string())?;
    state.stream.refresh_firewall();
    result
}

// ---- Print ------------------------------------------------------------------------------

/// Salva o PNG (corpo binário) em `Imagens\CapturaX` e devolve o caminho.
#[tauri::command]
pub async fn save_screenshot(app: AppHandle, request: Request<'_>) -> Result<String, String> {
    let data = raw_body(&request)?;
    if !data.starts_with(b"\x89PNG") {
        return Err("Invalid image".into());
    }
    let folder = app.path().picture_dir().map_err(|e| e.to_string())?.join("CapturaX");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut path = folder.join(format!("CapturaX {}.png", timestamp()));
    // Dois prints no mesmo segundo não podem se sobrescrever.
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("CapturaX {} ({n}).png", timestamp()));
        n += 1;
    }
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}
