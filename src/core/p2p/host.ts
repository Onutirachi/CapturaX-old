// Lado de quem transmite no P2P: fica conectado à sinalização e, para cada pessoa que abre o link,
// cria uma conexão WebRTC e manda o vídeo e o áudio direto para ela (o seu upload é dividido entre
// os espectadores; nada passa pela VPS).
import { ICE_SERVERS, signalUrl, type SignalData } from "./common";

export type P2PQuality = {
    /** Tamanho que sai (a fonte é reduzida se for maior). */
    height: number;
    sourceHeight: number;
    fps: number;
    /** bits/s */
    bitrate: number;
};

export type P2PState = "connecting" | "online" | "offline";

type HostMessage = { type: "ready"; room: string } | { type: "join" | "leave"; peer: number } | { type: "signal"; peer: number; data: SignalData };

const MAX_RETRY_MS = 30_000;

export class P2PHost {
    private ws: WebSocket | null = null;
    private peers = new Map<number, RTCPeerConnection>();
    private closed = false;
    private retry = 0;

    constructor(
        private server: string,
        private key: string,
        private video: MediaStreamTrack,
        private audio: MediaStreamTrack | null,
        private quality: P2PQuality,
        private maxViewers: number,
        private events: { onViewers: (count: number) => void; onState: (state: P2PState) => void }
    ) {
        this.connect();
    }

    private connect() {
        if (this.closed) return;
        this.events.onState("connecting");
        const ws = new WebSocket(`${signalUrl(this.server)}?role=host&key=${encodeURIComponent(this.key)}`);
        this.ws = ws;
        ws.onmessage = (event) => {
            try {
                this.handle(JSON.parse(event.data as string) as HostMessage);
            } catch (error) {
                console.error("P2P signal error", error);
            }
        };
        ws.onclose = () => {
            if (this.ws !== ws || this.closed) return;
            this.events.onState("offline");
            // Quem já está assistindo continua (o vídeo é direto); só novas entradas esperam.
            const delay = Math.min(MAX_RETRY_MS, 1000 * 2 ** this.retry++);
            window.setTimeout(() => this.connect(), delay);
        };
    }

    private send(message: object) {
        if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(message));
    }

    private handle(message: HostMessage) {
        switch (message.type) {
            case "ready":
                this.retry = 0;
                this.events.onState("online");
                break;
            case "join":
                this.addPeer(message.peer).catch((error) => console.error("P2P peer failed", error));
                break;
            case "leave":
                this.removePeer(message.peer);
                break;
            case "signal":
                this.signal(message.peer, message.data).catch((error) => console.error("P2P signal failed", error));
                break;
        }
    }

    private encoding(): RTCRtpEncodingParameters {
        const { height, sourceHeight, fps, bitrate } = this.quality;
        return { maxBitrate: bitrate, maxFramerate: fps, scaleResolutionDownBy: Math.max(1, sourceHeight / Math.max(1, height)) };
    }

    private async addPeer(peer: number) {
        if (this.peers.size >= this.maxViewers) {
            this.send({ type: "kick", peer });
            return;
        }
        const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
        this.peers.set(peer, pc);
        this.events.onViewers(this.peers.size);
        const stream = new MediaStream([this.video, ...(this.audio ? [this.audio] : [])]);
        const video = pc.addTransceiver(this.video, { direction: "sendonly", streams: [stream], sendEncodings: [this.encoding()] });
        // H.264 primeiro: o encoder da GPU e qualquer navegador tocam.
        const codecs = RTCRtpSender.getCapabilities("video")?.codecs ?? [];
        const h264 = codecs.filter((c) => c.mimeType.toLowerCase() === "video/h264");
        if (h264.length) video.setCodecPreferences([...h264, ...codecs.filter((c) => c.mimeType.toLowerCase() !== "video/h264")]);
        // Jogo: perder resolução antes de perder fluidez quando a rede apertar.
        const parameters = video.sender.getParameters();
        video.sender.setParameters({ ...parameters, degradationPreference: "maintain-framerate" } as RTCRtpSendParameters).catch(() => {});
        if (this.audio) pc.addTransceiver(this.audio, { direction: "sendonly", streams: [stream] });

        pc.onicecandidate = (event) => {
            if (event.candidate) this.send({ type: "signal", peer, data: { candidate: event.candidate.toJSON() } });
        };
        pc.onconnectionstatechange = () => {
            if (pc.connectionState === "failed" || pc.connectionState === "closed") this.removePeer(peer);
        };
        await pc.setLocalDescription(await pc.createOffer());
        this.send({ type: "signal", peer, data: { sdp: pc.localDescription!.toJSON() } });
    }

    private async signal(peer: number, data: SignalData) {
        const pc = this.peers.get(peer);
        if (!pc) return;
        if (data.sdp) await pc.setRemoteDescription(data.sdp);
        if (data.candidate) await pc.addIceCandidate(data.candidate).catch(() => {});
    }

    private removePeer(peer: number) {
        const pc = this.peers.get(peer);
        if (!pc) return;
        this.peers.delete(peer);
        pc.close();
        this.events.onViewers(this.peers.size);
    }

    stop() {
        this.closed = true;
        this.ws?.close();
        this.ws = null;
        for (const pc of this.peers.values()) pc.close();
        this.peers.clear();
        this.events.onViewers(0);
        this.events.onState("offline");
    }
}
