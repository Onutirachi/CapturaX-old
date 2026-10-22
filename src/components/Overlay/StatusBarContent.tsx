import { trackSettings } from "../../core/native/screenCapture";
import { useContext } from "preact/hooks";
import Tag from "../../core/components/Tag";
import Resolution from "../../core/utils/Resolution";
import { VideoStreamContext } from "../../context/VideoStreamContext";
import { FrameStatsContext } from "../../context/FrameStatsContext";
import { NativeContext } from "../../context/NativeContext";

import Icon from "./Icon";
import ButtonToggle from "../../core/components/ButtonToggle";
import InfoBubble from "./InfoBubble";
import Alerts from "./Alerts";
import { useT } from "../../i18n";

type ControlsProps = { fixed: boolean; setFixed: (value: boolean) => void };

function Controls({ fixed, setFixed }: ControlsProps) {
    return (
        <>
            <ButtonToggle on={() => setFixed(true)} off={() => setFixed(false)} storeState="statusBarFixed">
                <InfoBubble className="cursor-pointer">
                    {fixed ? <Icon src="./assets/StatusBar/pin_enable.svg" /> : <Icon src="./assets/StatusBar/pin_disable.svg" />}
                </InfoBubble>
            </ButtonToggle>
        </>
    );
}

function formatDuration(seconds: number) {
    const s = Math.floor(seconds);
    const hh = Math.floor(s / 3600);
    const mm = String(Math.floor((s % 3600) / 60)).padStart(2, "0");
    const ss = String(s % 60).padStart(2, "0");
    return hh > 0 ? `${hh}:${mm}:${ss}` : `${mm}:${ss}`;
}

function formatBytes(bytes: number) {
    if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} GB`;
    return `${Math.round(bytes / 1e6)} MB`;
}

/** Indicadores de gravação e transmissão em andamento. */
function MediaBubbles() {
    const t = useT();
    const nctx = useContext(NativeContext);
    if (!nctx) return null;
    const recording = nctx.status?.sessions.find((s) => s.kind === "recording");
    const streaming = nctx.status?.sessions.find((s) => s.kind === "stream");
    const viewers = (nctx.server?.viewers.length ?? 0) + nctx.p2pViewers;
    // "Tela 2 · 1920x1080 · 60 FPS"
    const describe = (kind: "record" | "stream") => {
        const output = nctx.outputs[kind];
        return output ? ` · ${output.label} · ${output.width}x${output.height} · ${output.fps} FPS` : "";
    };

    return (
        <div className="mr-auto flex gap-3 items-center min-w-0">
            {nctx.recording !== "idle" && (
                <InfoBubble>
                    <span className="w-3 h-3 rounded-full bg-bad animate-pulse" />
                    <span title={nctx.status?.recording?.path}>
                        {nctx.recording === "stopping" ? t("status.saving") : `REC ${formatDuration(recording?.elapsedSecs ?? 0)} · ${formatBytes(recording?.bytes ?? 0)}${describe("record")}`}
                    </span>
                </InfoBubble>
            )}
            {nctx.streaming !== "idle" && (
                <InfoBubble>
                    <span className="px-1.5 rounded-sm bg-accent text-sm font-semibold">{t("status.live")}</span>
                    <span>{`${formatDuration(streaming?.elapsedSecs ?? 0)} · ${t("status.watching", { count: viewers })}${describe("stream")}`}</span>
                </InfoBubble>
            )}
        </div>
    );
}

export default function StatusBarContent({ fixed, setFixed }: { fixed: boolean; setFixed: (value: boolean) => void }) {
    const t = useT();
    const vsctx = useContext(VideoStreamContext);
    const nctx = useContext(NativeContext);
    const { latency, latencyAvailable, fps: measuredFps } = useContext(FrameStatsContext)?.stats ?? { latency: null, latencyAvailable: false, fps: null };
    if (!vsctx) return null;
    const videoTrack = (nctx?.feedback ? nctx.outputTrack : null) ?? vsctx.videoTrack;

    if (!videoTrack) {
        return (
            <>
                <MediaBubbles />
                <Alerts />
                <Controls fixed={fixed} setFixed={setFixed} />
            </>
        );
    }

    const settings = trackSettings(videoTrack);
    const width = settings.width || 0;
    const height = settings.height || 0;
    const resolutionTag = Resolution.getStandardTag({ width, height });
    const aspectRatioTag = Resolution.getAspectRatio({ width, height });
    const isCapturing = videoTrack.readyState === "live" ? true : false;
    const configuredFps = settings.frameRate || 0;

    return (
        <>
            <MediaBubbles />
            <Alerts />
            <InfoBubble>
                {isCapturing ? (
                    <>
                        <Icon src="./assets/StatusBar/play.svg" />
                        <span>{t("status.capturing")}</span>
                    </>
                ) : (
                    <>
                        <Icon src="./assets/StatusBar/pause.svg" />
                        <span>{t("status.notCapturing")}</span>
                    </>
                )}
            </InfoBubble>
            <InfoBubble>
                <span>{`${settings.width} x ${settings.height}`}</span>
                {resolutionTag && <Tag text={resolutionTag.text} color={resolutionTag.color}></Tag>}
                {aspectRatioTag && <Tag text={aspectRatioTag}></Tag>}
            </InfoBubble>
            <InfoBubble className="min-w-24 tabular-nums">
                <span title={t("status.configuredFps", { fps: Math.round(configuredFps) })}>{`${Math.round(measuredFps ?? configuredFps)} FPS`}</span>
            </InfoBubble>
            {latencyAvailable && (
                <InfoBubble className="min-w-36 tabular-nums">
                    <span title={t("status.latencyHint")}>{latency !== null ? t("status.latency", { ms: Math.round(latency) }) : t("status.latencyUnknown")}</span>
                </InfoBubble>
            )}
            <Controls fixed={fixed} setFixed={setFixed} />
        </>
    );
}
