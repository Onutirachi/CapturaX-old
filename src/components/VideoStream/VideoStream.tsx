import { useContext, useEffect, useMemo, useRef } from "preact/hooks";
import { VideoStreamContext } from "../../context/VideoStreamContext";
import { FrameStatsContext, EMPTY_FRAME_STATS } from "../../context/FrameStatsContext";
import { NativeContext } from "../../context/NativeContext";
import AmbientLight from "./AmbientLight";
import { useT } from "../../i18n";

const STATS_UPDATE_MS = 500;
/** Janelas sem medição em que a última latência continua valendo (captura de tela só gera frame quando algo muda). */
const LATENCY_HOLD_WINDOWS = 6;

function median(values: number[]) {
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.floor(sorted.length / 2)];
}

export default function VideoStream() {
    const t = useT();
    const ctx = useContext(VideoStreamContext);
    const setStats = useContext(FrameStatsContext)?.setStats;
    const videoRef = useRef<HTMLVideoElement>(null);

    const volume = ctx?.volume ?? 1;
    const muted = ctx?.muted ?? false;
    // Preview oculto: o <video> fica sem fonte (o WebView para de desenhar), a captura continua.
    const hidden = ctx?.previewHidden ?? false;
    // "Ver gravação/live": o preview mostra o que está saindo em vez do dispositivo.
    const nctx = useContext(NativeContext);
    const feedbackTrack = nctx?.feedback ? nctx.outputTrack : null;
    const feedbackStream = useMemo(() => (feedbackTrack ? new MediaStream([feedbackTrack]) : null), [feedbackTrack]);
    const videoStream = hidden ? null : (feedbackStream ?? ctx?.videoStream ?? null);
    // Com o preview nativo, quem toca o áudio é o Rust (WASAPI); o <video> fica mudo.
    const nativeAudio = nctx?.config?.audio.nativePreview ?? true;

    // srcObject vai pelo efeito: como prop, o Preact não consegue zerar (tentaria "" e falharia).
    useEffect(() => {
        if (videoRef.current && videoRef.current.srcObject !== videoStream) videoRef.current.srcObject = videoStream;
    }, [videoStream]);

    useEffect(() => {
        if (!videoRef.current) return;
        videoRef.current.volume = volume;
        videoRef.current.muted = muted || nativeAudio;
    }, [volume, muted, nativeAudio]);

    // Estatísticas por frame via requestVideoFrameCallback:
    // - Latência = expectedDisplayTime - captureTime (do driver até a tela). Não inclui o atraso
    //   interno da câmera/placa antes do timestamp do driver.
    // - FPS = frames exibidos (presentedFrames) no intervalo / tempo decorrido.
    useEffect(() => {
        const video = videoRef.current;
        if (!video || !videoStream || !setStats) return;

        let latencySamples: number[] = [];
        let lastLatency: number | null = null;
        let windowsWithoutLatency = 0;
        let presentedFrames: number | null = null;
        let windowStartFrames: number | null = null;
        let windowStart = performance.now();
        let callbackId = 0;

        const onFrame: VideoFrameRequestCallback = (_, metadata) => {
            if (metadata.captureTime !== undefined) {
                latencySamples.push(metadata.expectedDisplayTime - metadata.captureTime);
            }
            presentedFrames = metadata.presentedFrames;
            windowStartFrames ??= presentedFrames;
            callbackId = video.requestVideoFrameCallback(onFrame);
        };
        callbackId = video.requestVideoFrameCallback(onFrame);

        const interval = window.setInterval(() => {
            const now = performance.now();
            const frames = presentedFrames !== null && windowStartFrames !== null ? presentedFrames - windowStartFrames : 0;
            if (latencySamples.length > 0) {
                lastLatency = median(latencySamples);
                windowsWithoutLatency = 0;
            } else {
                windowsWithoutLatency++;
            }
            setStats({
                latency: windowsWithoutLatency <= LATENCY_HOLD_WINDOWS ? lastLatency : null,
                latencyAvailable: lastLatency !== null,
                fps: frames > 0 ? (frames * 1000) / (now - windowStart) : null,
            });
            latencySamples = [];
            windowStartFrames = presentedFrames;
            windowStart = now;
        }, STATS_UPDATE_MS);

        return () => {
            video.cancelVideoFrameCallback(callbackId);
            window.clearInterval(interval);
            setStats(EMPTY_FRAME_STATS);
        };
    }, [videoStream, setStats]);

    if (!ctx) return;

    return (
        <div className="w-full h-full flex items-center justify-center relative">
            {hidden && (
                <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 text-(--text-muted) pointer-events-none">
                    <div className="w-10 h-10 mask-center mask-contain mask-no-repeat bg-(--text-muted) mask-[url(/assets/Dock/preview_off.svg)]" />
                    <span>{t("preview.hidden")}</span>
                    <span className="text-sm">{t("preview.hiddenHint")}</span>
                </div>
            )}
            {ctx.ambientLight && !hidden && <AmbientLight videoRef={videoRef} />}
            <video className={`h-full w-full object-contain transition relative`} autoPlay playsInline ref={videoRef} />
        </div>
    );
}
