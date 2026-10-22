// APIs do Chromium (WebView2) que ainda não estão nos tipos padrão do TypeScript.

declare class MediaStreamTrackProcessor<T = VideoFrame> {
    constructor(init: { track: MediaStreamTrack; maxBufferSize?: number });
    readonly readable: ReadableStream<T>;
}

declare class ImageCapture {
    constructor(track: MediaStreamTrack);
    grabFrame(): Promise<ImageBitmap>;
}

declare class MediaStreamTrackGenerator extends MediaStreamTrack {
    constructor(init: { kind: "video" | "audio" });
    readonly writable: WritableStream<VideoFrame | AudioData>;
}

/** Ponte do WebView2 com o app (só existe dentro do app). */
interface WebView2SharedBufferEvent extends Event {
    getBuffer(): ArrayBuffer;
    readonly additionalData: unknown;
}
interface WebView2MessageEvent extends Event {
    readonly data: unknown;
}
interface Window {
    chrome?: {
        webview?: {
            addEventListener(type: "message", listener: (event: WebView2MessageEvent) => void): void;
            addEventListener(type: "sharedbufferreceived", listener: (event: WebView2SharedBufferEvent) => void): void;
            removeEventListener(type: "message" | "sharedbufferreceived", listener: (event: never) => void): void;
            releaseBuffer?(buffer: ArrayBuffer): void;
        };
    };
}

/** Áudio cru do WebCodecs (para a trilha gerada do P2P). */
declare class AudioData {
    constructor(init: {
        format: "u8" | "s16" | "s32" | "f32" | "u8-planar" | "s16-planar" | "s32-planar" | "f32-planar";
        sampleRate: number;
        numberOfChannels: number;
        numberOfFrames: number;
        timestamp: number;
        data: BufferSource;
    });
    close(): void;
}
