// Áudio do P2P: a mistura do Rust (a mesma da gravação/transmissão) chega em blocos de 10 ms de PCM
// pela ponte do WebView2 e vira uma trilha de áudio comum, que o WebRTC manda junto com o vídeo.
import { native } from "../native/api";

type PcmMessage = { capturax: "pcm"; t: number; d: string };

const SAMPLE_RATE = 48_000;
const CHANNELS = 2;

const isPcm = (data: unknown): data is PcmMessage => typeof data === "object" && data !== null && (data as { capturax?: string }).capturax === "pcm";

function decode(base64: string) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return bytes;
}

export class P2PAudio {
    private generator = new MediaStreamTrackGenerator({ kind: "audio" });
    private writer = this.generator.writable.getWriter();
    private stopped = false;

    readonly track: MediaStreamTrack = this.generator;

    private onMessage = (event: WebView2MessageEvent) => {
        const data = event.data;
        if (!isPcm(data) || this.stopped) return;
        const pcm = decode(data.d);
        const frames = pcm.byteLength / 2 / CHANNELS;
        // Atrasado (aba em segundo plano etc.): descarta em vez de acumular atraso.
        if ((this.writer.desiredSize ?? 1) <= 0) return;
        const audio = new AudioData({ format: "s16", sampleRate: SAMPLE_RATE, numberOfChannels: CHANNELS, numberOfFrames: frames, timestamp: data.t, data: pcm });
        this.writer.write(audio).catch(() => audio.close());
    };

    async start() {
        window.chrome?.webview?.addEventListener("message", this.onMessage);
        await native.p2pAudioStart();
    }

    stop() {
        if (this.stopped) return;
        this.stopped = true;
        window.chrome?.webview?.removeEventListener("message", this.onMessage as (event: never) => void);
        native.p2pAudioStop().catch(() => {});
        this.writer.close().catch(() => {});
        this.generator.stop();
    }
}
