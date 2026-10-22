// Player P2P da página de quem assiste (`/v/<sala>`): conversa com a sinalização, recebe a oferta
// de quem transmite e toca o vídeo/áudio que chega direto pelo WebRTC. Se a transmissão ainda não
// começou (ou acabou), espera conectado e entra sozinho quando ela voltar.
import { ICE_SERVERS, signalUrl, type SignalData } from "../src/core/p2p/common";

type ViewerMessage = { type: "waiting" | "live" | "ended" | "full" | "kicked" } | { type: "signal"; data: SignalData };

type Events = {
    /** Está ao vivo e o vídeo começou a chegar (hora de dar play). */
    onPlaying: (stream: MediaStream) => void;
    /** Fora do ar (esperando a transmissão). */
    onOffline: () => void;
    /** Sala cheia ou recusado por quem transmite. */
    onFull: () => void;
};

const RETRY_MS = 3000;

/** Opus em estéreo e com bitrate de música (o padrão do navegador é mono, para voz). */
function stereoOpus(sdp: string) {
    const payload = sdp.match(/a=rtpmap:(\d+) opus\/48000/i)?.[1];
    if (!payload) return sdp;
    return sdp.replace(new RegExp(`a=fmtp:${payload} ([^\\r\\n]*)`), (_, params: string) => `a=fmtp:${payload} ${params};stereo=1;sprop-stereo=1;maxaveragebitrate=128000`);
}

export class P2PPlayer {
    private ws: WebSocket | null = null;
    private pc: RTCPeerConnection | null = null;
    private closed = false;

    constructor(
        private server: string,
        private room: string,
        private events: Events
    ) {
        this.connect();
    }

    private connect() {
        if (this.closed) return;
        const ws = new WebSocket(`${signalUrl(this.server)}?role=viewer&room=${this.room}`);
        this.ws = ws;
        ws.onmessage = (event) => {
            try {
                this.handle(JSON.parse(event.data as string) as ViewerMessage);
            } catch (error) {
                console.error("signal error", error);
            }
        };
        ws.onclose = () => {
            if (this.ws !== ws || this.closed) return;
            this.closePeer();
            this.events.onOffline();
            window.setTimeout(() => this.connect(), RETRY_MS);
        };
    }

    private send(data: SignalData) {
        if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ type: "signal", data }));
    }

    private handle(message: ViewerMessage) {
        switch (message.type) {
            case "waiting":
            case "ended":
                this.closePeer();
                this.events.onOffline();
                break;
            case "full":
            case "kicked":
                this.closed = true;
                this.closePeer();
                this.ws?.close();
                this.events.onFull();
                break;
            case "signal":
                this.signal(message.data).catch((error) => console.error("WebRTC error", error));
                break;
        }
    }

    private async signal(data: SignalData) {
        if (data.sdp?.type === "offer") {
            // Oferta nova (início ou transmissão reiniciada): conexão nova.
            this.closePeer();
            const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
            this.pc = pc;
            const stream = new MediaStream();
            pc.ontrack = (event) => {
                stream.addTrack(event.track);
                if (event.track.kind === "video") this.events.onPlaying(stream);
            };
            pc.onicecandidate = (event) => {
                if (event.candidate) this.send({ candidate: event.candidate.toJSON() });
            };
            pc.onconnectionstatechange = () => {
                if (this.pc === pc && pc.connectionState === "failed") {
                    this.closePeer();
                    this.events.onOffline();
                }
            };
            await pc.setRemoteDescription(data.sdp);
            const answer = await pc.createAnswer();
            await pc.setLocalDescription({ type: "answer", sdp: stereoOpus(answer.sdp ?? "") });
            this.send({ sdp: pc.localDescription!.toJSON() });
        }
        if (data.candidate && this.pc) await this.pc.addIceCandidate(data.candidate).catch(() => {});
    }

    private closePeer() {
        this.pc?.close();
        this.pc = null;
    }

    destroy() {
        this.closed = true;
        this.closePeer();
        this.ws?.close();
        this.ws = null;
    }
}
