// Worker de codificação: lê frames da trilha de vídeo, reduz resolução/FPS se preciso e
// codifica na GPU (VideoEncoder com prefer-hardware). Fora da thread da interface para não
// atrasar o preview.
import type { VideoColor } from "../native/api";
import { encoderConfig, type EncoderSettings } from "./presets";

export type ToWorker =
    | { type: "start"; readable: ReadableStream<VideoFrame>; settings: EncoderSettings }
    | { type: "source"; readable: ReadableStream<VideoFrame> }
    | { type: "stop" };

export type FromWorker =
    | { type: "config"; codec: string; description: ArrayBuffer; width: number; height: number; color: VideoColor | null }
    | { type: "chunk"; data: ArrayBuffer; absUs: number; key: boolean }
    | { type: "error"; message: string }
    | { type: "stopped"; dropped: number };

const post = (message: FromWorker, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(message, transfer);
const nowAbsUs = () => (performance.timeOrigin + performance.now()) * 1000;

let settings: EncoderSettings;
let encoder: VideoEncoder | null = null;
let reader: ReadableStreamDefaultReader<VideoFrame> | null = null;
let canvas: OffscreenCanvas | null = null;
let context: OffscreenCanvasRenderingContext2D | null = null;
let configSent = false;
let stopped = false;
let dropped = 0;

// Tempo dos frames: `frame.timestamp` só vale como diferença; ancoramos no relógio absoluto
// quando o primeiro frame de cada fonte chega.
let anchorOffset: number | null = null;
let lastAbsUs = -Infinity;
/** Os frames vão ao encoder com tempo relativo (começa em 0); somamos de volta na saída. */
let baseAbsUs: number | null = null;
let lastKeptUs = -Infinity;
let lastKeyUs = -Infinity;

function toBytes(source: AllowSharedBufferSource): ArrayBuffer {
    if (source instanceof ArrayBuffer) return source.slice(0);
    if (ArrayBuffer.isView(source)) return source.buffer.slice(source.byteOffset, source.byteOffset + source.byteLength) as ArrayBuffer;
    return new Uint8Array(source as SharedArrayBuffer).slice().buffer;
}

// Códigos da ISO/IEC 23091-2 para os nomes do WebCodecs (2 = não especificado).
const PRIMARIES: Record<string, number> = { bt709: 1, bt470bg: 5, smpte170m: 6, bt2020: 9, smpte432: 12 };
const TRANSFER: Record<string, number> = { bt709: 1, smpte170m: 6, linear: 8, "iec61966-2-1": 13, pq: 16, hlg: 18 };
const MATRIX: Record<string, number> = { rgb: 0, bt709: 1, bt470bg: 5, smpte170m: 6, "bt2020-ncl": 9 };

/**
 * Cor do vídeo como o encoder informou. Placas em MJPEG entregam faixa completa (0–255) e matriz
 * BT.601; o encoder codifica assim mas não escreve no vídeo, então o Rust escreve.
 */
function videoColor(space: VideoColorSpaceInit | undefined): VideoColor | null {
    if (!space || space.fullRange == null) return null;
    return {
        primaries: PRIMARIES[space.primaries ?? ""] ?? 2,
        transfer: TRANSFER[space.transfer ?? ""] ?? 2,
        matrix: MATRIX[space.matrix ?? ""] ?? 2,
        fullRange: space.fullRange,
    };
}

function createEncoder() {
    encoder = new VideoEncoder({
        output: (chunk, metadata) => {
            // O HEVC do Chromium não manda `description` (vem em Annex B); o Rust monta a partir
            // do primeiro keyframe. Por isso a config vai no primeiro pacote, com ou sem ela.
            const description = metadata?.decoderConfig?.description;
            if (!configSent) {
                configSent = true;
                const copy = description ? toBytes(description) : new ArrayBuffer(0);
                const color = videoColor(metadata?.decoderConfig?.colorSpace);
                post({ type: "config", codec: settings.codec, description: copy, width: settings.width, height: settings.height, color }, [copy]);
            }
            const data = new ArrayBuffer(chunk.byteLength);
            chunk.copyTo(data);
            post({ type: "chunk", data, absUs: chunk.timestamp + baseAbsUs!, key: chunk.type === "key" }, [data]);
        },
        error: (error) => post({ type: "error", message: error.message }),
    });
    encoder.configure(encoderConfig(settings));
}

function scaled(frame: VideoFrame, timestamp: number): VideoFrame {
    if (frame.displayWidth === settings.width && frame.displayHeight === settings.height) {
        return new VideoFrame(frame, { timestamp });
    }
    if (!canvas) {
        canvas = new OffscreenCanvas(settings.width, settings.height);
        context = canvas.getContext("2d", { alpha: false })!;
        context.imageSmoothingQuality = "high";
    }
    // Mantém a proporção (barras pretas se a fonte mudar de formato no meio).
    const scale = Math.min(settings.width / frame.displayWidth, settings.height / frame.displayHeight);
    const w = frame.displayWidth * scale;
    const h = frame.displayHeight * scale;
    context!.fillStyle = "#000";
    context!.fillRect(0, 0, settings.width, settings.height);
    context!.drawImage(frame, (settings.width - w) / 2, (settings.height - h) / 2, w, h);
    return new VideoFrame(canvas, { timestamp });
}

function handle(frame: VideoFrame) {
    if (stopped || !encoder || encoder.state !== "configured") {
        frame.close();
        return;
    }
    anchorOffset ??= nowAbsUs() - frame.timestamp;
    // Sempre crescente, mesmo se a fonte trocar e o relógio dela começar de outro ponto.
    const absUs = Math.max(frame.timestamp + anchorOffset, lastAbsUs + 1);
    lastAbsUs = absUs;

    // Limite de FPS (com folga para relógios que oscilam um pouco).
    if (absUs - lastKeptUs < (1_000_000 / settings.fps) * 0.85) {
        frame.close();
        return;
    }
    // GPU não está dando conta: descarta em vez de acumular atraso.
    if (encoder.encodeQueueSize > 4) {
        dropped++;
        frame.close();
        return;
    }
    lastKeptUs = absUs;

    const key = absUs - lastKeyUs >= settings.keyframeSecs * 1_000_000;
    if (key) lastKeyUs = absUs;
    baseAbsUs ??= absUs;
    const input = scaled(frame, absUs - baseAbsUs);
    frame.close();
    encoder.encode(input, { keyFrame: key });
    input.close();
}

async function pump(readable: ReadableStream<VideoFrame>) {
    const current = readable.getReader();
    reader = current;
    anchorOffset = null;
    try {
        while (!stopped && reader === current) {
            const { value, done } = await current.read();
            if (done) break;
            handle(value);
        }
    } catch {
        // trilha encerrada
    } finally {
        current.releaseLock();
    }
}

self.onmessage = async (event: MessageEvent<ToWorker>) => {
    const message = event.data;
    switch (message.type) {
        case "start":
            settings = message.settings;
            try {
                createEncoder();
            } catch (error) {
                post({ type: "error", message: String(error) });
                return;
            }
            pump(message.readable);
            break;
        case "source":
            await reader?.cancel().catch(() => {});
            pump(message.readable);
            break;
        case "stop":
            stopped = true;
            await reader?.cancel().catch(() => {});
            if (encoder && encoder.state === "configured") {
                await encoder.flush().catch(() => {});
                encoder.close();
            }
            post({ type: "stopped", dropped });
            break;
    }
};
