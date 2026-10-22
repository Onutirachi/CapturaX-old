// Qualidade de gravação/transmissão → configuração do VideoEncoder (WebCodecs).
import type { RecordingConfig, StreamConfig, VideoCodecName } from "../native/api";
import { t } from "../../i18n";

export type EncoderSettings = {
    codec: string;
    width: number;
    height: number;
    fps: number;
    bitrate: number;
    /** Intervalo entre keyframes; define o tamanho dos fragmentos/segmentos. */
    keyframeSecs: number;
    /**
     * Informar o FPS ao encoder. O Chromium recusa FPS acima de 30 em resoluções maiores que
     * 1080p no encoder de hardware do Windows, mas codifica normalmente se o FPS for omitido.
     */
    declareFramerate?: boolean;
};

/** Sem `framerate`, o encoder assume 30 fps e gasta o bitrate por frame como se fossem 30. */
const ASSUMED_FPS = 30;

export type SourceInfo = { width: number; height: number; fps: number };

const even = (n: number) => Math.max(2, Math.round(n / 2) * 2);

/** Reduz para no máximo `maxHeight` mantendo a proporção da fonte. */
function fitHeight(source: SourceInfo, maxHeight: number) {
    if (source.height <= maxHeight) return { width: even(source.width), height: even(source.height) };
    return { width: even((source.width * maxHeight) / source.height), height: even(maxHeight) };
}

/** Bits por pixel por frame: quanto maior, melhor a imagem e maior o arquivo. */
function bitrateFor(width: number, height: number, fps: number, bitsPerPixel: number) {
    return Math.round(Math.min(150_000_000, Math.max(2_000_000, width * height * fps * bitsPerPixel)));
}

// Níveis H.264: (amostras de luma por segundo, macroblocos por frame).
const AVC_LEVELS: [number, number, number][] = [
    [0x28, 62_914_560, 8_192], // 4.0
    [0x2a, 133_693_440, 8_704], // 4.2
    [0x32, 150_994_944, 22_080], // 5.0
    [0x33, 251_658_240, 36_864], // 5.1
    [0x34, 530_841_600, 36_864], // 5.2
    [0x3c, 4_278_190_080, 139_264], // 6.0
    [0x3e, 16_711_680_000, 139_264], // 6.2
];

// Níveis HEVC (nível × 30) com o limite de amostras de luma por segundo.
const HEVC_LEVELS: [number, number][] = [
    [120, 66_846_720], // 4
    [123, 133_693_440], // 4.1
    [150, 267_386_880], // 5
    [153, 534_773_760], // 5.1
    [156, 1_069_547_520], // 5.2
    [180, 1_069_547_520], // 6
    [183, 2_139_095_040], // 6.1
    [186, 4_278_190_080], // 6.2
];

export function codecString(codec: VideoCodecName, width: number, height: number, fps: number) {
    const samples = width * height * fps;
    if (codec === "hevc") {
        const level = HEVC_LEVELS.find(([, max]) => samples <= max)?.[0] ?? 186;
        return `hvc1.1.6.L${level}.B0`;
    }
    const macroblocks = Math.ceil(width / 16) * Math.ceil(height / 16);
    const level = AVC_LEVELS.find(([, max, mbs]) => samples <= max && macroblocks <= mbs)?.[0] ?? 0x3e;
    return `avc1.6400${level.toString(16).padStart(2, "0")}`;
}

export function recordingSettings(config: RecordingConfig, source: SourceInfo): EncoderSettings {
    const sourceFps = Math.max(1, Math.round(source.fps || 60));
    let size: { width: number; height: number };
    let fps: number;
    let bitrate: number;

    switch (config.preset) {
        case "original":
            size = fitHeight(source, Infinity);
            fps = sourceFps;
            bitrate = bitrateFor(size.width, size.height, fps, 0.12);
            break;
        case "high":
            size = fitHeight(source, 1080);
            fps = sourceFps;
            bitrate = bitrateFor(size.width, size.height, fps, 0.1);
            break;
        case "medium":
            size = fitHeight(source, 720);
            fps = Math.min(sourceFps, 60);
            bitrate = bitrateFor(size.width, size.height, fps, 0.08);
            break;
        case "light":
            size = fitHeight(source, 720);
            fps = Math.min(sourceFps, 30);
            bitrate = bitrateFor(size.width, size.height, fps, 0.07);
            break;
        case "custom":
            size = fitBox(source, config.width || Infinity, config.height || Infinity);
            fps = Math.min(sourceFps, config.fps ?? sourceFps);
            bitrate = config.bitrateKbps * 1000;
            break;
    }
    return { codec: codecString(config.codec, size.width, size.height, fps), ...size, fps, bitrate, keyframeSecs: 2 };
}

// Bitrate automático da transmissão (kbps) por altura, a 30 e a 60 fps.
const STREAM_BITRATES: [number, number, number][] = [
    [480, 2000, 3200],
    [720, 4000, 6000],
    [1080, 6500, 10000],
    [1440, 11000, 16000],
    [2160, 20000, 30000],
];

/** Teto do automático (a partir daí, só no manual). */
const MAX_AUTO_STREAM_KBPS = 50_000;

/**
 * Bitrate automático da transmissão para a resolução/FPS escolhidos. A tabela dá 30 e 60 fps; entre
 * eles interpola, e acima de 60 cresce um pouco menos que o FPS (quadros seguidos se parecem mais),
 * para cada quadro não ficar com menos bits: 1080p a 144 fps ≈ 20 Mbps. Medido: com menos que isso
 * o encoder da GPU desmancha em blocos quando a câmera gira rápido.
 */
export function autoStreamBitrateKbps(height: number, fps: number) {
    const [, at30, at60] = STREAM_BITRATES.find(([h]) => height <= h) ?? STREAM_BITRATES[STREAM_BITRATES.length - 1];
    const kbps = fps <= 30 ? at30 : fps <= 60 ? at30 + ((at60 - at30) * (fps - 30)) / 30 : at60 * Math.pow(fps / 60, 0.8);
    return Math.min(MAX_AUTO_STREAM_KBPS, Math.round(kbps / 100) * 100);
}

/** Cabe dentro de `maxWidth`×`maxHeight` mantendo a proporção da fonte (nunca aumenta). */
function fitBox(source: SourceInfo, maxWidth: number, maxHeight: number) {
    const scale = Math.min(1, maxWidth / source.width, maxHeight / source.height);
    return { width: even(source.width * scale), height: even(source.height * scale) };
}

/** Tamanho e FPS reais da transmissão para esta fonte (0 na config = nativo). */
export function streamSize(config: StreamConfig, source: SourceInfo) {
    const size = fitBox(source, config.width || Infinity, config.height || Infinity);
    const sourceFps = Math.max(1, Math.round(source.fps || 60));
    const fps = config.fps ? Math.min(sourceFps, config.fps) : sourceFps;
    return { ...size, fps };
}

export function streamBitrateKbps(config: StreamConfig, source: SourceInfo) {
    if (!config.bitrateAuto) return config.bitrateKbps;
    const { height, fps } = streamSize(config, source);
    return autoStreamBitrateKbps(height, fps);
}

export function streamSettings(config: StreamConfig, source: SourceInfo): EncoderSettings {
    const { width, height, fps } = streamSize(config, source);
    // Transmissão sempre em H.264: é o que todo navegador toca. Keyframe a cada 2 s: cada um custa
    // caro, e com 1 s sobrava menos bit para os quadros de movimento (quem entra recebe as partes
    // desde o último keyframe, então não espera por ele).
    return { codec: codecString("h264", width, height, fps), width, height, fps, bitrate: streamBitrateKbps(config, source) * 1000, keyframeSecs: 2 };
}

/** Configuração completa do VideoEncoder para estas opções. */
export function encoderConfig(settings: EncoderSettings): VideoEncoderConfig {
    const declare = settings.declareFramerate !== false;
    const base: VideoEncoderConfig = {
        codec: settings.codec,
        width: settings.width,
        height: settings.height,
        bitrate: declare ? settings.bitrate : Math.round((settings.bitrate * ASSUMED_FPS) / settings.fps),
        ...(declare ? { framerate: settings.fps } : {}),
        hardwareAcceleration: "prefer-hardware",
        latencyMode: "realtime",
        bitrateMode: "variable",
    };
    if (settings.codec.startsWith("avc")) return { ...base, avc: { format: "avc" } } as VideoEncoderConfig;
    return { ...base, hevc: { format: "hevc" } } as VideoEncoderConfig;
}

async function supported(settings: EncoderSettings) {
    const result = await VideoEncoder.isConfigSupported(encoderConfig(settings)).catch(() => ({ supported: false }));
    return !!result.supported;
}

/** Usa o codec pedido se a máquina suportar (com ou sem declarar o FPS); senão cai para H.264. */
export async function resolveSupported(settings: EncoderSettings): Promise<EncoderSettings> {
    const codecs = [settings.codec];
    if (!settings.codec.startsWith("avc")) codecs.push(codecString("h264", settings.width, settings.height, settings.fps));
    for (const codec of codecs) {
        for (const declareFramerate of [true, false]) {
            const candidate = { ...settings, codec, declareFramerate };
            if (await supported(candidate)) return candidate;
        }
    }
    throw new Error(t("encoder.unsupported", { size: `${settings.width}x${settings.height}`, fps: settings.fps }));
}

export async function isHevcSupported() {
    if (typeof VideoEncoder === "undefined") return false;
    const settings: EncoderSettings = { codec: codecString("hevc", 1920, 1080, 60), width: 1920, height: 1080, fps: 60, bitrate: 10_000_000, keyframeSecs: 2 };
    const result = await VideoEncoder.isConfigSupported(encoderConfig(settings)).catch(() => ({ supported: false }));
    return !!result.supported;
}
