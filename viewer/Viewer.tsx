// Página de quem assiste: toca a transmissão do CapturaX com baixa latência (MediaSource), mostra
// "Offline" fora do ar e entra sozinha quando a transmissão começa. Sem MediaSource (Safari no
// iPhone), usa o HLS nativo, com mais atraso. Controles só afetam este navegador.
import { useEffect, useRef, useState } from "preact/hooks";
import { LowLatencyPlayer } from "./player";
import { P2PPlayer } from "./p2pPlayer";
import { roomFromPath } from "../src/core/p2p/common";
import AmbientLight from "../src/components/VideoStream/AmbientLight";
import Icon from "../src/components/Overlay/Icon";
import logoUrl from "../public/assets/Brand/logo.svg?url";
import fullscreenUrl from "../public/assets/Dock/fullscreen.svg?url";
import fullscreenExitUrl from "../public/assets/Dock/fullscreen_exit.svg?url";
import ambientUrl from "../public/assets/Dock/ambient_light.svg?url";
import volumeHighUrl from "../public/assets/Dock/volume_high.svg?url";
import volumeLowUrl from "../public/assets/Dock/volume_low.svg?url";
import volumeMuteUrl from "../public/assets/Dock/volume_mute.svg?url";
import playUrl from "../public/assets/StatusBar/play.svg?url";
import { t } from "../src/i18n";

type LiveStatus = { live: boolean; session: string; codecs?: string };
const OFFLINE: LiveStatus = { live: false, session: "" };

const STATUS_MS = 2000;
/** Link P2P (`/v/<sala>`, servido pela VPS): vídeo direto de quem transmite, sem o servidor do app. */
const P2P_ROOM = roomFromPath(location.pathname);
const IDLE_MS = 2500;

function stored<T>(key: string, fallback: T): T {
    try {
        const value = localStorage.getItem(key);
        return value === null ? fallback : (JSON.parse(value) as T);
    } catch {
        return fallback;
    }
}

function save(key: string, value: unknown) {
    try {
        localStorage.setItem(key, JSON.stringify(value));
    } catch {
        // modo privado: só não lembra
    }
}

export default function Viewer() {
    const videoRef = useRef<HTMLVideoElement>(null);
    const [status, setStatus] = useState<LiveStatus | null>(null);
    const [full, setFull] = useState(false);
    const [volume, setVolume] = useState<number>(() => stored("volume", 1));
    const [muted, setMuted] = useState<boolean>(() => stored("muted", false));
    const [ambient, setAmbient] = useState<boolean>(() => stored("ambientLight", true));
    const [needsClick, setNeedsClick] = useState(false);
    const [idle, setIdle] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // P2P: a sinalização avisa quando está ao vivo e o vídeo chega direto.
    useEffect(() => {
        const video = videoRef.current;
        if (!P2P_ROOM || !video) return;
        const player = new P2PPlayer(location.host, P2P_ROOM, {
            onPlaying: (stream) => {
                video.srcObject = stream;
                setStatus({ live: true, session: "p2p" });
                video.play().catch(() => {
                    video.muted = true;
                    setNeedsClick(true);
                    video.play().catch(() => {});
                });
            },
            onOffline: () => {
                video.srcObject = null;
                setStatus(OFFLINE);
            },
            onFull: () => {
                video.srcObject = null;
                setStatus(OFFLINE);
                setError(t("viewer.full"));
            },
        });
        return () => player.destroy();
    }, []);

    // Consulta se está ao vivo; a sessão muda a cada transmissão.
    useEffect(() => {
        if (P2P_ROOM) return;
        let alive = true;
        const check = async () => {
            try {
                const response = await fetch("/live/status", { cache: "no-store" });
                if (!response.ok) throw new Error(response.status === 503 ? "full" : String(response.status));
                const next: LiveStatus = await response.json();
                if (alive) setStatus((current) => (current?.live === next.live && current.session === next.session ? current : next));
                setError(null);
            } catch {
                if (alive) setStatus(OFFLINE);
            }
        };
        check();
        const interval = window.setInterval(check, STATUS_MS);
        return () => {
            alive = false;
            window.clearInterval(interval);
        };
    }, []);

    // Liga o player quando entra ao vivo (ou quando começa outra transmissão).
    useEffect(() => {
        const video = videoRef.current;
        if (P2P_ROOM || !video || !status?.live) return;
        const base = `/live/${status.session}`;

        const play = () =>
            video.play().catch(() => {
                // Navegador bloqueou som automático: começa mudo e pede um clique.
                video.muted = true;
                setNeedsClick(true);
                video.play().catch(() => {});
            });
        // A conexão caiu: volta a "Offline" e a próxima consulta reconecta no ao vivo.
        const reconnect = () => setStatus(OFFLINE);

        let player: LowLatencyPlayer | null = null;
        if (status.codecs && LowLatencyPlayer.supports(status.codecs)) {
            player = new LowLatencyPlayer(video, `${base}/live.mp4`, status.codecs, {
                onReady: play,
                onEnded: reconnect,
                onFull: () => setError(t("viewer.full")),
            });
        } else if (video.canPlayType("application/vnd.apple.mpegurl")) {
            video.src = `${base}/index.m3u8`; // Safari / iPhone
            video.addEventListener("loadedmetadata", play, { once: true });
            video.addEventListener("error", reconnect, { once: true });
        } else {
            setError(t("viewer.unsupported"));
        }
        return () => {
            if (player) player.destroy();
            else {
                video.removeAttribute("src");
                video.load();
            }
        };
    }, [status?.live, status?.session]);

    useEffect(() => {
        const video = videoRef.current;
        if (!video) return;
        video.volume = volume;
        if (!needsClick) video.muted = muted;
        save("volume", volume);
        save("muted", muted);
    }, [volume, muted, needsClick]);

    useEffect(() => save("ambientLight", ambient), [ambient]);

    useEffect(() => {
        const sync = () => setFull(!!document.fullscreenElement);
        document.addEventListener("fullscreenchange", sync);
        return () => document.removeEventListener("fullscreenchange", sync);
    }, []);

    // Controles somem com o mouse parado.
    useEffect(() => {
        let timer = window.setTimeout(() => setIdle(true), IDLE_MS);
        const wake = () => {
            setIdle(false);
            window.clearTimeout(timer);
            timer = window.setTimeout(() => setIdle(true), IDLE_MS);
        };
        window.addEventListener("pointermove", wake);
        window.addEventListener("keydown", wake);
        return () => {
            window.clearTimeout(timer);
            window.removeEventListener("pointermove", wake);
            window.removeEventListener("keydown", wake);
        };
    }, []);

    const toggleFullscreen = () => (document.fullscreenElement ? document.exitFullscreen() : document.documentElement.requestFullscreen()).catch(() => {});
    const unmute = () => {
        setNeedsClick(false);
        setMuted(false);
        if (videoRef.current) {
            videoRef.current.muted = false;
            videoRef.current.play().catch(() => {});
        }
    };

    const live = !!status?.live;
    const volumeIcon = muted || volume === 0 ? volumeMuteUrl : volume < 0.5 ? volumeLowUrl : volumeHighUrl;
    const button = "w-11 h-11 flex items-center justify-center rounded-xl hover:bg-normal cursor-pointer transition";

    return (
        <main className={`fixed inset-0 bg-gradient overflow-hidden font-family-segoe text-white ${idle && live ? "cursor-none" : ""}`} onDblClick={toggleFullscreen}>
            {live && ambient && <AmbientLight videoRef={videoRef} />}
            <video ref={videoRef} playsInline className={`absolute inset-0 w-full h-full object-contain ${live ? "" : "invisible"}`} />

            {!live && (
                <div className="absolute inset-0 flex flex-col items-center justify-center gap-6 p-6 text-center">
                    <img src={logoUrl} alt="CapturaX" className="h-10 select-none" draggable={false} />
                    <div className="flex items-center gap-2 text-lg">
                        <span className="w-2.5 h-2.5 rounded-full bg-(--text-muted)" />
                        {t(status === null ? "viewer.connecting" : "viewer.offline")}
                    </div>
                    <p className="text-(--text-muted) max-w-sm">{error ?? t("viewer.waiting")}</p>
                </div>
            )}

            {live && needsClick && (
                <button onClick={unmute} className="absolute inset-0 m-auto w-max h-max flex items-center gap-3 px-6 py-4 rounded-2xl bg-accent text-lg font-semibold shadow-2xl cursor-pointer hover:brightness-110">
                    <div className="w-6 h-6 flex items-center justify-center">
                        <Icon src={playUrl} />
                    </div>
                    {t("viewer.clickForSound")}
                </button>
            )}

            {live && (
                <div
                    className={`absolute inset-x-0 bottom-0 flex items-center gap-2 p-3 bg-linear-to-t from-black/70 to-transparent transition-opacity duration-300 ${
                        idle ? "opacity-0" : "opacity-100"
                    }`}>
                    <span className="px-2 py-0.5 rounded-md bg-accent text-sm font-semibold">{t("status.live")}</span>
                    <img src={logoUrl} alt="" className="h-3.5 ml-2 opacity-80 select-none" draggable={false} />
                    <div className="flex-1" />
                    <button title={t(muted ? "dock.unmute" : "dock.mute")} onClick={() => (needsClick ? unmute() : setMuted(!muted))} className={button}>
                        <Icon src={volumeIcon} />
                    </button>
                    <input
                        type="range"
                        min={0}
                        max={1}
                        step={0.01}
                        value={muted ? 0 : volume}
                        onInput={(e) => {
                            setVolume(Number(e.currentTarget.value));
                            setMuted(false);
                            if (needsClick) unmute();
                        }}
                        className="w-28 accent-accent cursor-pointer"
                        title={t("viewer.volume")}
                    />
                    <button title={t(ambient ? "dock.ambientOff" : "dock.ambientOn")} onClick={() => setAmbient(!ambient)} className={`${button} ${ambient ? "bg-normal" : ""}`}>
                        <Icon src={ambientUrl} color={ambient ? "var(--color-accent)" : undefined} />
                    </button>
                    <button title={t("viewer.fullscreen")} onClick={toggleFullscreen} className={button}>
                        <Icon src={full ? fullscreenExitUrl : fullscreenUrl} />
                    </button>
                </div>
            )}
        </main>
    );
}
