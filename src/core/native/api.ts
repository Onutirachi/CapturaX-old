// Ponte tipada para os comandos Rust (src-tauri/src/commands.rs).
import { invoke } from "@tauri-apps/api/core";
import type { VideoDeviceModes } from "../utils/videoModes";

/** `containerId`: aparelho físico (vídeo e áudio de uma mesma placa USB têm o mesmo). */
export type DeviceInfo = { id: string; name: string; isDefault: boolean; containerId: string | null };
export type AudioDevices = { inputs: DeviceInfo[]; outputs: DeviceInfo[] };

export type AudioConfig = {
    nativePreview: boolean;
    previewInput: string | null;
    /** Usa o áudio do mesmo aparelho do vídeo (o Rust resolve com `audioSetPairedInput`). */
    previewInputAuto: boolean;
    previewOutput: string | null;
    previewDelayMs: number;
    recordInput: string | null;
    recordInputEnabled: boolean;
    recordInputVolume: number;
    recordMic: string | null;
    recordMicVolume: number;
    /** Gravações: mistura + "sem o mic" + "só o mic" em faixas separadas. */
    recordMicSeparate: boolean;
    recordSystem: boolean;
    recordSystemVolume: number;
    excludedApps: string[];
    knownApps: string[];
    avOffsetMs: number;
};

export type RecordingPreset = "original" | "high" | "medium" | "light" | "custom";
export type VideoCodecName = "h264" | "hevc";

/** Fonte da gravação/transmissão: `null` = a mesma do preview; senão `screen:...`/`window:...`. */
export type OutputSourceId = string | null;

export type RecordingConfig = {
    folder: string | null;
    preset: RecordingPreset;
    codec: VideoCodecName;
    width: number | null;
    height: number | null;
    fps: number | null;
    bitrateKbps: number;
    audioBitrateKbps: number;
};

export type StreamConfig = {
    acceptViewers: boolean;
    port: number;
    upnp: boolean;
    width: number;
    height: number;
    fps: number;
    /** Bitrate calculado pela resolução/FPS; `false` usa `bitrateKbps`. */
    bitrateAuto: boolean;
    bitrateKbps: number;
    audioBitrateKbps: number;
    blockedIps: string[];
    /** Link direto (servido por este PC). Desligado, nem codifica para ele. */
    directEnabled: boolean;
    /** Link P2P (WebRTC): quem assiste conecta direto, sem abrir porta. */
    p2pEnabled: boolean;
    /** Servidor de sinalização (`relay/`), ex.: `meucapturax.duckdns.org`. Vazio = sem P2P. */
    p2pServer: string;
    /** Senha de uso ou de admin desse servidor. */
    p2pPassword: string;
    /** Segredo deste app; a sala do link é `sha256(segredo)`. */
    p2pKey: string;
    p2pMaxViewers: number;
};

export type Config = {
    /** Idioma da interface (`"pt-BR"`); `null` = o do Windows. */
    language: string | null;
    /** Fonte da gravação e da transmissão (as duas usam a mesma). */
    outputSource: OutputSourceId;
    audio: AudioConfig;
    recording: RecordingConfig;
    stream: StreamConfig;
};

export type AppListEntry = { exe: string; name: string; running: boolean; playing: boolean; excluded: boolean };

export type SessionStatus = { id: number; kind: "recording" | "stream"; elapsedSecs: number; bytes: number };
export type MediaStatus = { sessions: SessionStatus[]; recording: { id: number; path: string } | null; streaming: number | null };

export type UpnpStatus =
    | { state: "disabled" }
    | { state: "searching" }
    | { state: "mapped"; externalIp: string; cgnat: boolean }
    | { state: "failed"; reason: string };

export type ViewerInfo = { ip: string; watchingSecs: number };

export type ServerStatus = {
    running: boolean;
    port: number;
    localUrl: string | null;
    publicUrl: string | null;
    /** Link pelo IPv6 público do PC (dispensa abrir porta; quem assiste precisa de IPv6). */
    publicUrlV6: string | null;
    upnp: UpnpStatus;
    /** IP público desta conexão (consulta DNS). */
    publicIp: string | null;
    /** Atrás de CGNAT: abrir a porta não adianta. */
    cgnat: boolean;
    firewallOk: boolean;
    live: boolean;
    viewers: ViewerInfo[];
    error: string | null;
};

export type ScreenSource = { id: string; number: number; name: string; width: number; height: number; refreshRate: number; primary: boolean };
export type WindowSource = { id: string; title: string; app: string; width: number; height: number };
export type CaptureSources = { screens: ScreenSource[]; windows: WindowSource[] };

export const native = {
    clockNow: () => invoke<number>("clock_now"),
    getConfig: () => invoke<Config>("get_config"),
    setConfig: (config: Config) => invoke<void>("set_config", { config }),

    audioDevices: () => invoke<AudioDevices>("audio_devices"),
    setPreviewVolume: (volume: number, muted: boolean) => invoke<void>("audio_set_preview_volume", { volume, muted }),
    audioApps: () => invoke<AppListEntry[]>("audio_apps"),
    systemLanguages: () => invoke<string[]>("system_languages"),
    p2pAudioStart: () => invoke<void>("p2p_audio_start"),
    p2pAudioStop: () => invoke<void>("p2p_audio_stop"),
    audioSetPairedInput: (id: string | null) => invoke<void>("audio_set_paired_input", { id }),
    audioSetOutputPairedInput: (id: string | null) => invoke<void>("audio_set_output_paired_input", { id }),
    videoModes: () => invoke<VideoDeviceModes[]>("video_modes"),
    captureSources: () => invoke<CaptureSources>("capture_sources"),
    captureStart: (id: string, fps: number, cursor: boolean) => invoke<{ width: number; height: number }>("capture_start", { id, fps, cursor }),
    captureSetFps: (fps: number) => invoke<void>("capture_set_fps", { fps }),
    captureStop: () => invoke<void>("capture_stop"),

    recordingStart: () => invoke<{ id: number; path: string }>("recording_start"),
    recordingStop: () => invoke<string | null>("recording_stop"),
    streamStart: () => invoke<number>("stream_start"),
    streamStop: () => invoke<void>("stream_stop"),
    streamPrepare: () => invoke<void>("stream_prepare"),
    streamUnprepare: () => invoke<void>("stream_unprepare"),
    mediaStatus: () => invoke<MediaStatus>("media_status"),

    videoConfig: (session: number, codec: string, description: Uint8Array, width: number, height: number) =>
        invoke<void>("media_video_config", { session, codec, description: Array.from(description), width, height }),
    /** Pacote de vídeo codificado; vai como corpo binário (sem JSON) e os metadados nos cabeçalhos. */
    videoChunk: (session: number, data: Uint8Array, ptsUs: number, key: boolean) =>
        invoke<void>("media_video_chunk", data, {
            headers: { "x-session": String(session), "x-pts": String(Math.round(ptsUs)), "x-key": key ? "1" : "0" },
        }),

    streamStatus: () => invoke<ServerStatus>("stream_status"),
    firewallFix: () => invoke<void>("firewall_fix"),

    saveScreenshot: (png: Uint8Array) => invoke<string>("save_screenshot", png),
};

/** Mensagem de erro legível de uma rejeição do `invoke`. */
/** Texto do erro no idioma atual (erros do Rust vêm com código; ver `src/i18n`). */
export { errorText } from "../../i18n";
