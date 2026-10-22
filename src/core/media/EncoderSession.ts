// Liga uma trilha de vídeo ao Worker de codificação e manda os pacotes para uma sessão do Rust.
import { native, errorText } from "../native/api";
import { absToQpcUs } from "../native/clock";
import type { EncoderSettings } from "./presets";
import type { FromWorker, ToWorker } from "./encoder.worker";

export default class EncoderSession {
    private worker: Worker;
    /** Envio em fila: os pacotes precisam chegar ao Rust na ordem. */
    private sending: Promise<void> = Promise.resolve();
    private configured: Promise<void>;
    private resolveConfigured!: () => void;
    private stopped: Promise<number>;
    private resolveStopped!: (dropped: number) => void;
    /** Clone da trilha que alimenta o encoder (precisa ser encerrado ao trocar/parar). */
    private source: MediaStreamTrack | null = null;

    constructor(
        private sessionId: number,
        settings: EncoderSettings,
        track: MediaStreamTrack,
        private onError: (message: string) => void
    ) {
        this.configured = new Promise((resolve) => (this.resolveConfigured = resolve));
        this.stopped = new Promise((resolve) => (this.resolveStopped = resolve));
        this.worker = new Worker(new URL("./encoder.worker.ts", import.meta.url), { type: "module" });
        this.worker.onmessage = (event: MessageEvent<FromWorker>) => this.handle(event.data);
        this.post({ type: "start", readable: this.readable(track), settings });
    }

    /** Clona a trilha: parar o encoder não pode parar o preview. */
    private readable(track: MediaStreamTrack) {
        this.source?.stop();
        this.source = track.clone();
        return new MediaStreamTrackProcessor({ track: this.source, maxBufferSize: 2 }).readable;
    }

    private post(message: ToWorker) {
        const transfer = "readable" in message ? [message.readable as unknown as Transferable] : [];
        this.worker.postMessage(message, transfer);
    }

    private handle(message: FromWorker) {
        switch (message.type) {
            case "config":
                this.sending = this.sending
                    .then(() => native.videoConfig(this.sessionId, message.codec, new Uint8Array(message.description), message.width, message.height, message.color))
                    .then(() => this.resolveConfigured())
                    .catch((error) => this.onError(errorText(error)));
                break;
            case "chunk":
                this.sending = this.sending
                    .then(() => this.configured)
                    .then(() => absToQpcUs(message.absUs))
                    .then((pts) => native.videoChunk(this.sessionId, new Uint8Array(message.data), pts, message.key))
                    .catch((error) => console.error("Video chunk lost", error));
                break;
            case "error":
                this.onError(message.message);
                break;
            case "stopped":
                if (message.dropped > 0) console.warn(`Encoder dropped ${message.dropped} frames (GPU busy)`);
                this.resolveStopped(message.dropped);
                break;
        }
    }

    /** Troca a fonte (ex.: dispositivo reaberto) sem reiniciar o arquivo. */
    setTrack(track: MediaStreamTrack) {
        this.post({ type: "source", readable: this.readable(track) });
    }

    /** Termina de codificar e espera todos os pacotes chegarem ao Rust. */
    async stop() {
        this.post({ type: "stop" });
        await Promise.race([this.stopped, new Promise((resolve) => setTimeout(resolve, 5000))]);
        this.worker.terminate();
        this.source?.stop();
        this.source = null;
        await this.sending;
    }
}
