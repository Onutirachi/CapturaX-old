//! Configurações que o Rust precisa conhecer, salvas em `settings.json` na pasta de config do app.
//! Preferências só de interface (dropdowns, pin da StatusBar…) continuam no localStorage do front.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Idioma da interface (`"pt-BR"`); `None` = o do Windows.
    pub language: Option<String>,
    /// Fonte da gravação e da transmissão (as duas usam a mesma): `None` = a do preview;
    /// `screen:...`/`window:...` = tela/janela.
    pub output_source: Option<String>,
    pub audio: AudioConfig,
    pub recording: RecordingConfig,
    pub stream: StreamConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioConfig {
    /// `true` = preview tocado pelo Rust (WASAPI, baixa latência); `false` = pelo `<video>` do WebView.
    pub native_preview: bool,
    /// Endpoint WASAPI de entrada do preview (ex. áudio da placa). `None` = nenhum.
    pub preview_input: Option<String>,
    /// Usa o áudio do mesmo aparelho do vídeo (ContainerId) em vez de `preview_input`.
    pub preview_input_auto: bool,
    /// Endpoint de saída do preview. `None` = saída padrão do Windows.
    pub preview_output: Option<String>,
    /// Atraso extra do áudio do preview, para alinhar com o vídeo (lip-sync).
    pub preview_delay_ms: u32,

    /// Dispositivo gravado junto (ex. áudio da placa). `None` = automático: o áudio do mesmo
    /// aparelho do vídeo do preview (independente do que se está ouvindo).
    pub record_input: Option<String>,
    pub record_input_enabled: bool,
    pub record_input_volume: f32,
    /// Microfone opcional na gravação/transmissão.
    pub record_mic: Option<String>,
    pub record_mic_volume: f32,
    /// Gravações com o microfone também em trilhas separadas (para editar depois).
    pub record_mic_separate: bool,
    /// Áudio dos apps (loopback app por app, sem o CapturaX e sem os excluídos).
    pub record_system: bool,
    pub record_system_volume: f32,
    /// Executáveis excluídos do áudio do sistema, em minúsculas (ex. `discord.exe`).
    pub excluded_apps: Vec<String>,
    /// Apps já vistos tocando som, para a lista continuar útil com eles fechados.
    pub known_apps: Vec<String>,
    /// Ajuste fino de sincronia da gravação: positivo atrasa o áudio.
    pub av_offset_ms: i32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            native_preview: true,
            preview_input: None,
            preview_input_auto: true,
            preview_output: None,
            preview_delay_ms: 0,
            record_input: None,
            record_input_enabled: true,
            record_input_volume: 1.0,
            record_mic: None,
            record_mic_volume: 1.0,
            record_mic_separate: false,
            record_system: true,
            record_system_volume: 1.0,
            excluded_apps: vec!["discord.exe".into()],
            known_apps: vec!["discord.exe".into()],
            av_offset_ms: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct RecordingConfig {
    /// Pasta de destino. `None` = `Vídeos\CapturaX`.
    pub folder: Option<String>,
    /// "original" | "high" | "medium" | "light" | "custom"
    pub preset: String,
    /// "h264" | "hevc"
    pub codec: String,
    /// Usados só no preset "custom". `None` = igual ao preview.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            folder: None,
            preset: "high".into(),
            codec: "h264".into(),
            width: None,
            height: None,
            fps: None,
            bitrate_kbps: 20_000,
            audio_bitrate_kbps: 192,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct StreamConfig {
    /// Servidor no ar (link responde, mesmo que "Offline").
    pub accept_viewers: bool,
    pub port: u16,
    /// Pedir ao roteador para abrir a porta (UPnP).
    pub upnp: bool,
    /// Tamanho máximo (mantém a proporção da fonte); 0 = nativo.
    pub width: u32,
    pub height: u32,
    /// 0 = nativo.
    pub fps: u32,
    /// Bitrate calculado pela resolução/FPS; `false` usa `bitrate_kbps`.
    pub bitrate_auto: bool,
    pub bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub blocked_ips: Vec<String>,
    /// Link P2P (WebRTC): quem assiste conecta direto, sem abrir porta. A VPS só apresenta os dois.
    pub p2p_enabled: bool,
    /// Servidor de sinalização (`relay/`), ex.: `capturax.duckdns.org`.
    pub p2p_server: String,
    /// Segredo deste app; a sala do link é `sha256(segredo)`. Gerado pelo app na primeira vez.
    pub p2p_key: String,
    /// Cada espectador P2P usa o seu upload inteiro do vídeo: limite de pessoas ao mesmo tempo.
    pub p2p_max_viewers: u32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            accept_viewers: false,
            port: 8787,
            upnp: true,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_auto: true,
            bitrate_kbps: 4_500,
            audio_bitrate_kbps: 128,
            blocked_ips: Vec::new(),
            p2p_enabled: true,
            p2p_server: "capturax.duckdns.org".into(),
            p2p_key: String::new(),
            p2p_max_viewers: 5,
        }
    }
}

pub struct ConfigStore {
    path: PathBuf,
    pub current: Config,
}

impl ConfigStore {
    pub fn load(dir: PathBuf) -> Self {
        let path = dir.join("settings.json");
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| match serde_json::from_str(&text) {
                Ok(config) => Some(config),
                Err(error) => {
                    // Arquivo corrompido não pode impedir o app de abrir.
                    eprintln!("Ignoring invalid {}: {error}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        Self { path, current }
    }

    pub fn save(&self) {
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Grava num temporário e renomeia: uma queda no meio não deixa o arquivo pela metade.
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&self.current)?)?;
            std::fs::rename(&tmp, &self.path)
        })();
        if let Err(error) = result {
            eprintln!("Failed to save settings: {error}");
        }
    }
}
