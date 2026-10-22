import { trackSettings } from "../core/native/screenCapture";
import { ComponentChildren, createContext } from "preact";
import { useCallback, useContext, useEffect, useRef, useState } from "preact/hooks";
import { native, errorText, type AudioDevices, type CaptureSources, type Config, type MediaStatus, type OutputSourceId, type ServerStatus } from "../core/native/api";
import { acquireCapture } from "../core/native/captureHub";
import { nativeCaptureAvailable } from "../core/native/screenCapture";
import { deviceSource, openDeviceTrack, sourceDeviceId, sourceExists, sourceLabel, sourceRefreshRate } from "../core/utils/outputSources";
import { holdDevice, releaseDevice, trackDeviceId } from "../core/native/deviceShare";
import { findDevice, pairedAudio } from "../core/utils/videoModes";
import { calibrateClock } from "../core/native/clock";
import EncoderSession from "../core/media/EncoderSession";
import { recordingSettings, resolveSupported, streamSettings, type SourceInfo } from "../core/media/presets";
import { alerts, fileName } from "../core/utils/alerts";
import { AlertsContext } from "./AlertsContext";
import { VideoStreamContext } from "./VideoStreamContext";
import { applyLanguageSetting, t } from "../i18n";
import { P2PHost, type P2PState } from "../core/p2p/host";
import { P2PAudio } from "../core/p2p/audio";
import { claimName, newP2PKey, p2pLink, type ClaimError } from "../core/p2p/common";

type Activity = "idle" | "starting" | "active" | "stopping";
export type OutputKind = "record" | "stream";

/** O que uma gravação/transmissão em andamento está mandando. */
export type OutputInfo = { source: OutputSourceId; track: MediaStreamTrack; label: string; width: number; height: number; fps: number };

/** Trilha aberta para uma saída (tela/janela própria, ou a do preview). */
type OpenOutput = { source: OutputSourceId; track: MediaStreamTrack; release: () => void };
export type SettingsTab = "general" | "audio" | "recording" | "streaming";

export type NativeContextType = {
    config: Config | null;
    /** Atualiza na hora na interface e salva no Rust logo em seguida (agrupando mudanças seguidas). */
    updateConfig: (update: (config: Config) => Config) => void;
    devices: AudioDevices;
    status: MediaStatus | null;
    /** Servidor da transmissão (só consultado quando relevante). */
    server: ServerStatus | null;
    recording: Activity;
    streaming: Activity;
    /** F10: grava/para direto, com as últimas escolhas da janela. */
    toggleRecording: () => Promise<void>;
    /** Janela "Gravar / Transmitir": fonte, áudio e qualidade das duas saídas. */
    outputDialogOpen: boolean;
    openOutputDialog: () => void;
    closeOutputDialog: () => void;
    /** Começa a gravação e/ou a transmissão (as que ainda não estiverem rodando). */
    startOutputs: (which: { record: boolean; stream: boolean }) => Promise<void>;
    /** Ao vivo: aplica a qualidade nova (reinicia a transmissão). */
    applyStreamQuality: () => Promise<void>;
    stopRecording: () => Promise<void>;
    /** Ao vivo, mudou fonte, áudio ou qualidade desde que a transmissão começou ("Aplicar" reinicia). */
    streamDirty: boolean;
    /** Link P2P (sem abrir porta), se o servidor estiver configurado. */
    p2pLink: string | null;
    /** Por que não há link P2P (apelido em uso, senha errada…). */
    p2pLinkError: ClaimError | null;
    /** Pessoas assistindo pelo P2P agora. */
    p2pViewers: number;
    /** Conexão com o servidor de sinalização (só enquanto transmite). */
    p2pState: P2PState;
    /** Fonte da saída: a escolhida, ou o dispositivo do preview quando nada foi escolhido ainda. */
    outputSource: OutputSourceId;
    /** Entrada de áudio que o "Auto" do áudio gravado usa agora (null = nenhuma). */
    outputAutoAudioId: string | null;
    /** Telas e janelas disponíveis como fonte da gravação/transmissão. */
    captureSources: CaptureSources;
    refreshCaptureSources: () => void;
    /** Gravação/transmissão em andamento (fonte, tamanho, FPS). */
    outputs: Record<OutputKind, OutputInfo | null>;
    /** Preview mostrando o que sai (menu de cima travado). Só quando a fonte não é o preview. */
    feedback: boolean;
    setFeedback: (on: boolean) => void;
    /** Trilha da saída (tela/janela) quando a fonte não é o preview. */
    outputTrack: MediaStreamTrack | null;
    stopStreaming: () => Promise<void>;
    screenshot: () => Promise<void>;
    settingsTab: SettingsTab | null;
    openSettings: (tab?: SettingsTab | null) => void;
};

/** O que, se mudar ao vivo, só vale reiniciando a transmissão: fonte, dispositivos de áudio e qualidade. */
function streamKey(config: Config, source: OutputSourceId) {
    const { recordInput, recordInputEnabled, recordMic, recordSystem } = config.audio;
    const { width, height, fps, bitrateAuto, bitrateKbps, p2pEnabled, p2pServer, p2pPassword, p2pNickname, p2pKey, maxViewers } = config.stream;
    return JSON.stringify([source, recordInput, recordInputEnabled, recordMic, recordSystem, width, height, fps, bitrateAuto, bitrateKbps, p2pEnabled, p2pServer, p2pPassword, p2pNickname, p2pKey, maxViewers]);
}

/** P2P configurado (servidor + senha) e ligado. */
export const p2pConfigured = (config: Config) => !!config.stream.p2pServer && !!config.stream.p2pPassword;
export const p2pActive = (config: Config) => config.stream.p2pEnabled && p2pConfigured(config) && !!config.stream.p2pKey;

export const NativeContext = createContext<NativeContextType | null>(null);

const SAVE_DELAY_MS = 250;

function sourceInfo(track: MediaStreamTrack): SourceInfo {
    const settings = trackSettings(track);
    return { width: settings.width ?? 1920, height: settings.height ?? 1080, fps: settings.frameRate ?? 60 };
}

export function NativeProvider({ children }: { children: ComponentChildren }) {
    const vsctx = useContext(VideoStreamContext);
    const setAlert = useContext(AlertsContext)?.setAlert;
    const videoTrack = vsctx?.videoTrack ?? null;

    const [config, setConfig] = useState<Config | null>(null);
    const [devices, setDevices] = useState<AudioDevices>({ inputs: [], outputs: [] });
    const [status, setStatus] = useState<MediaStatus | null>(null);
    const [recording, setRecording] = useState<Activity>("idle");
    const [streaming, setStreaming] = useState<Activity>("idle");
    const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
    const [server, setServer] = useState<ServerStatus | null>(null);
    const [outputDialogOpen, setOutputDialogOpen] = useState(false);
    const [captureSources, setCaptureSources] = useState<CaptureSources>({ screens: [], windows: [] });
    const [outputs, setOutputs] = useState<Record<OutputKind, OutputInfo | null>>({ record: null, stream: null });
    const [feedback, setFeedbackState] = useState(false);
    const [p2pName, setP2PName] = useState<{ name: string } | { error: ClaimError } | null>(null);
    const [p2pViewers, setP2PViewers] = useState(0);
    const [p2pState, setP2PState] = useState<P2PState>("offline");
    const p2p = useRef<{ host: P2PHost; audio: P2PAudio } | null>(null);
    /** O que a transmissão em andamento usa (para saber se algo mudou e precisa "Aplicar"). */
    const [streamSnapshot, setStreamSnapshot] = useState<string | null>(null);
    const opened = useRef<Record<OutputKind, OpenOutput | null>>({ record: null, stream: null });
    const sourcesRef = useRef(captureSources);
    sourcesRef.current = captureSources;

    const configRef = useRef<Config | null>(null);
    const feedbackRef = useRef(false);
    feedbackRef.current = feedback;
    const saveTimer = useRef<number | null>(null);
    const recorder = useRef<EncoderSession | null>(null);
    const streamer = useRef<EncoderSession | null>(null);
    const trackRef = useRef<MediaStreamTrack | null>(null);
    trackRef.current = videoTrack;

    const alert = useCallback((value: Parameters<NonNullable<typeof setAlert>>[0]) => setAlert?.(value), [setAlert]);

    useEffect(() => {
        native
            .getConfig()
            .then((loaded) => {
                configRef.current = loaded;
                setConfig(loaded);
            })
            .catch((error) => alert(alerts.error(t("native.loadFailed", { detail: errorText(error) }))));
        calibrateClock().catch(() => {});
    }, []);

    const updateConfig = useCallback((update: (config: Config) => Config) => {
        if (!configRef.current) return;
        const next = update(configRef.current);
        configRef.current = next;
        setConfig(next);
        if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
        saveTimer.current = window.setTimeout(() => {
            saveTimer.current = null;
            native.setConfig(configRef.current!).catch((error) => alert(alerts.error(t("native.saveFailed", { detail: errorText(error) }))));
        }, SAVE_DELAY_MS);
    }, []);

    const refreshCaptureSources = useCallback(() => {
        if (nativeCaptureAvailable()) native.captureSources().then(setCaptureSources).catch(() => {});
    }, []);
    useEffect(refreshCaptureSources, []);

    // Fonte: a escolhida; sem escolha (ou se sumiu), o dispositivo que está no preview.
    const previewDeviceId = trackDeviceId(videoTrack);
    const videoDevices = vsctx?.videoDevices ?? [];
    const devicesRef = useRef(videoDevices);
    devicesRef.current = videoDevices;
    const sourcesLoaded = captureSources.screens.length > 0 && videoDevices.length > 0;
    const savedSource = config?.outputSource ?? null;
    const outputSource: OutputSourceId =
        savedSource && (!sourcesLoaded || sourceExists(savedSource, captureSources, videoDevices)) ? savedSource : previewDeviceId ? deviceSource(previewDeviceId) : null;
    const outputSourceRef = useRef(outputSource);
    outputSourceRef.current = outputSource;

    /** Abre a fonte de uma saída (dispositivo, tela ou janela). Gravação e transmissão usam a mesma. */
    const openOutput = async (kind: OutputKind, fps: number | null, onEnded: () => void): Promise<OpenOutput | null> => {
        const source = outputSourceRef.current;
        if (!source) return null;
        const deviceId = sourceDeviceId(source);
        if (deviceId !== null) {
            const modes = findDevice(vsctx?.allDeviceModes ?? [], devicesRef.current.find((d) => d.deviceId === deviceId)?.label ?? "")?.modes ?? null;
            const track = await openDeviceTrack(deviceId, modes);
            holdDevice(deviceId, track);
            track.addEventListener("ended", onEnded);
            const release = () => {
                releaseDevice(deviceId, track);
                track.removeEventListener("ended", onEnded);
                track.stop();
            };
            opened.current[kind] = { source, track, release };
            return opened.current[kind];
        }
        // A lista pode estar velha (janela aberta agora): atualiza antes de abrir.
        const sources = await native.captureSources().catch(() => sourcesRef.current);
        setCaptureSources(sources);
        const { track, release } = await acquireCapture(source, fps ?? sourceRefreshRate(source, sources), onEnded);
        opened.current[kind] = { source, track, release };
        return opened.current[kind];
    };

    // "Auto" do áudio gravado: o do aparelho da fonte (tela/janela: o do preview, que o Rust usa
    // quando isto é `null`).
    const outputDeviceId = sourceDeviceId(outputSource);
    const outputPaired =
        outputDeviceId !== null
            ? pairedAudio(findDevice(vsctx?.allDeviceModes ?? [], videoDevices.find((d) => d.deviceId === outputDeviceId)?.label ?? ""), devices.inputs)?.id ?? ""
            : null;
    useEffect(() => {
        // "" = dispositivo sem áudio (o Rust não deve cair no do preview).
        native.audioSetOutputPairedInput(outputPaired === "" ? "none" : outputPaired).catch(() => {});
    }, [outputPaired]);

    const closeOutput = (kind: OutputKind) => {
        opened.current[kind]?.release();
        opened.current[kind] = null;
        setOutputs((current) => ({ ...current, [kind]: null }));
        if (!opened.current.record && !opened.current.stream) setFeedbackState(false);
    };

    const describeOutput = (kind: OutputKind, out: OpenOutput, settings: { width: number; height: number; fps: number }) =>
        setOutputs((current) => ({
            ...current,
            [kind]: { source: out.source, track: out.track, label: sourceLabel(out.source, sourcesRef.current, devicesRef.current), ...settings },
        }));

    const outputTrack = opened.current.stream?.track ?? opened.current.record?.track ?? null;
    const setFeedback = (on: boolean) => setFeedbackState(on && !!(opened.current.stream || opened.current.record));

    // Dispositivos de áudio do Windows; a lista do WebView avisa quando algo é plugado.
    useEffect(() => {
        const refresh = () =>
            native
                .audioDevices()
                .then(setDevices)
                .catch((error) => console.error("Could not list audio devices", error));
        refresh();
        navigator.mediaDevices.addEventListener("devicechange", refresh);
        return () => navigator.mediaDevices.removeEventListener("devicechange", refresh);
    }, []);

    useEffect(() => {
        const poll = () =>
            native
                .mediaStatus()
                .then(setStatus)
                .catch(() => {});
        poll();
        const interval = window.setInterval(poll, 1000);
        return () => window.clearInterval(interval);
    }, []);

    const watchServer = streaming !== "idle" || outputDialogOpen || settingsTab === "streaming" || !!config?.stream.acceptViewers;
    useEffect(() => {
        if (!watchServer) return;
        const poll = () =>
            native
                .streamStatus()
                .then(setServer)
                .catch(() => {});
        poll();
        const interval = window.setInterval(poll, 2000);
        return () => window.clearInterval(interval);
    }, [watchServer]);

    // P2P: segredo criado na primeira vez (o link fica fixo); a sala é derivada dele.
    const p2pKey = config?.stream.p2pKey;
    useEffect(() => {
        if (config && !p2pKey) updateConfig((c) => ({ ...c, stream: { ...c.stream, p2pKey: newP2PKey() } }));
    }, [p2pKey, !!config]);

    // Reserva o nome do link no servidor (com uma pausa, para não perguntar a cada tecla).
    const p2pServer = config?.stream.p2pServer ?? "";
    const p2pPassword = config?.stream.p2pPassword ?? "";
    const p2pNickname = config?.stream.p2pNickname ?? "";
    useEffect(() => {
        if (!p2pServer || !p2pPassword || !p2pKey) return setP2PName(null);
        let current = true;
        const timer = window.setTimeout(() => {
            claimName(p2pServer, p2pPassword, p2pKey, p2pNickname).then((result) => current && setP2PName(result));
        }, 700);
        return () => {
            current = false;
            window.clearTimeout(timer);
        };
    }, [p2pServer, p2pPassword, p2pKey, p2pNickname]);

    // Idioma: o escolhido nas configurações ou o do Windows.
    const language = config?.language;
    useEffect(() => {
        if (config) applyLanguageSetting(language ?? null);
    }, [language, !!config]);

    // Volume do preview nativo acompanha o slider da Dock.
    const volume = vsctx?.volume ?? 1;
    const muted = vsctx?.muted ?? false;
    useEffect(() => {
        native.setPreviewVolume(volume, muted).catch(() => {});
    }, [volume, muted]);


    const startRecording = async () => {
        const current = configRef.current;
        if (!current) return;
        setRecording("starting");
        let out: OpenOutput | null = null;
        try {
            const fps = current.recording.preset === "custom" ? current.recording.fps : null;
            out = await openOutput("record", fps, () => {
                alert(alerts.warning(t("native.recordSourceEnded")));
                stopRecordingRef.current();
            });
            if (!out) {
                setRecording("idle");
                alert(alerts.warning(t("native.nothingToRecord")));
                return;
            }
            const track = out.track;
            const settings = await resolveSupported(recordingSettings(current.recording, sourceInfo(track)));
            await calibrateClock();
            const { id, path } = await native.recordingStart();
            recorder.current = new EncoderSession(id, settings, track, (message) => alert(alerts.error(t("native.recordingError", { detail: message }))));
            describeOutput("record", out, settings);
            setRecording("active");
            alert(alerts.info(t("native.recordingStarted", { size: `${settings.width}x${settings.height}`, fps: settings.fps, file: fileName(path) })));
        } catch (error) {
            setRecording("idle");
            closeOutput("record");
            alert(alerts.error(t("native.recordingStartFailed", { detail: errorText(error) })));
        }
    };

    const stopRecording = async () => {
        setRecording("stopping");
        try {
            await recorder.current?.stop();
            recorder.current = null;
            const path = await native.recordingStop();
            alert(path ? alerts.success(t("native.recordingSaved", { file: fileName(path) })) : alerts.warning(t("native.recordingEmpty")));
        } catch (error) {
            alert(alerts.error(t("native.recordingStopFailed", { detail: errorText(error) })));
        } finally {
            closeOutput("record");
            setRecording("idle");
        }
    };
    const stopRecordingRef = useRef(stopRecording);
    stopRecordingRef.current = stopRecording;

    const startStreaming = async (restarting = false) => {
        const current = configRef.current;
        if (!current) return;
        setStreaming("starting");
        try {
            const out = await openOutput("stream", current.stream.fps || null, () => {
                alert(alerts.warning(t("native.streamSourceEnded")));
                stopStreamingRef.current();
            });
            if (!out) {
                setStreaming("idle");
                alert(alerts.warning(t("native.nothingToStream")));
                return;
            }
            const track = out.track;
            const settings = await resolveSupported(streamSettings(current.stream, sourceInfo(track)));
            const { p2pServer, p2pPassword, p2pKey, p2pNickname, maxViewers } = current.stream;
            const p2pOn = p2pActive(current);
            // Link direto: codifica e serve por este PC. No P2P o WebRTC codifica (um encoder só).
            if (!p2pOn) {
                await calibrateClock();
                const id = await native.streamStart();
                streamer.current = new EncoderSession(id, settings, track, (message) => alert(alerts.error(t("native.streamError", { detail: message }))));
            }
            describeOutput("stream", out, settings);
            setStreamSnapshot(streamKey(current, outputSourceRef.current));
            if (p2pOn) {
                const audio = new P2PAudio();
                await audio.start().catch((error) => console.error("P2P audio failed", error));
                const host = new P2PHost(
                    p2pServer,
                    p2pKey,
                    p2pPassword,
                    p2pNickname,
                    track,
                    audio.track,
                    { height: settings.height, sourceHeight: sourceInfo(track).height, fps: settings.fps, bitrate: settings.bitrate },
                    maxViewers,
                    {
                        onViewers: setP2PViewers,
                        onState: (state) => {
                            setP2PState(state);
                            if (state === "unauthorized") alert(alerts.error(t("p2p.unauthorized")));
                            if (state === "name_taken") alert(alerts.error(t("p2p.nameTaken")));
                        },
                    }
                );
                p2p.current = { host, audio };
            }
            setStreaming("active");
            alert(alerts.info(restarting ? t("native.streamUpdated", { size: `${settings.width}x${settings.height}`, fps: settings.fps }) : t("native.live")));
        } catch (error) {
            setStreaming("idle");
            closeOutput("stream");
            alert(alerts.error(t("native.streamStartFailed", { detail: errorText(error) })));
        }
    };

    const stopP2P = () => {
        p2p.current?.host.stop();
        p2p.current?.audio.stop();
        p2p.current = null;
    };

    const stopStreaming = async () => {
        stopP2P();
        setStreaming("stopping");
        try {
            await streamer.current?.stop();
            streamer.current = null;
            await native.streamStop();
            alert(alerts.info(t("native.streamEnded")));
        } catch (error) {
            alert(alerts.error(t("native.streamStopFailed", { detail: errorText(error) })));
        } finally {
            closeOutput("stream");
            setStreaming("idle");
        }
    };
    const stopStreamingRef = useRef(stopStreaming);
    stopStreamingRef.current = stopStreaming;

    const busy = (activity: Activity) => activity === "starting" || activity === "stopping";
    const toggleRecording = async () => {
        if (busy(recording)) return;
        await (recording === "active" ? stopRecording() : startRecording());
    };

    /** Garante que o que foi escolhido na janela já foi salvo no Rust antes de começar. */
    const flushConfig = async () => {
        if (saveTimer.current === null) return;
        window.clearTimeout(saveTimer.current);
        saveTimer.current = null;
        await native.setConfig(configRef.current!).catch(() => {});
    };

    const openOutputDialog = () => {
        refreshCaptureSources();
        setOutputDialogOpen(true);
        // Liga o servidor já, para o link (e a porta no roteador) estar pronto na janela.
        // Liga o servidor do link direto já, para o link (e a porta no roteador) estar pronto na janela.
        if (streaming !== "active" && configRef.current && !p2pActive(configRef.current)) native.streamPrepare().catch(() => {});
    };
    const closeOutputDialog = () => {
        setOutputDialogOpen(false);
        if (streaming === "idle") native.streamUnprepare().catch(() => {});
    };
    const startOutputs = async (which: { record: boolean; stream: boolean }) => {
        await flushConfig();
        setOutputDialogOpen(false);
        const tasks: Promise<void>[] = [];
        if (which.record && recording === "idle") tasks.push(startRecording());
        if (which.stream && streaming === "idle") tasks.push(startStreaming());
        await Promise.all(tasks);
        if (which.stream === false && streaming === "idle") native.streamUnprepare().catch(() => {});
    };

    /** Nova qualidade ao vivo: o formato do vídeo muda, então a transmissão recomeça (quem assiste
     * reconecta sozinho em 1–2 s). O servidor fica no ar durante a troca. */
    const restartStreaming = async () => {
        stopP2P();
        setStreaming("starting");
        try {
            await native.streamPrepare();
            await streamer.current?.stop();
            streamer.current = null;
            await native.streamStop();
        } catch (error) {
            alert(alerts.error(t("native.streamUpdateFailed", { detail: errorText(error) })));
        }
        closeOutput("stream");
        await startStreaming(true);
    };
    const applyStreamQuality = async () => {
        if (streaming !== "active") return;
        await flushConfig();
        setOutputDialogOpen(false);
        await restartStreaming();
    };

    const screenshot = async () => {
        const track = (feedbackRef.current && (opened.current.stream?.track ?? opened.current.record?.track)) || trackRef.current;
        if (!track) {
            alert(alerts.warning(t("native.nothingToCapture")));
            return;
        }
        try {
            const bitmap = await new ImageCapture(track).grabFrame();
            const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
            canvas.getContext("2d")!.drawImage(bitmap, 0, 0);
            bitmap.close();
            const blob = await canvas.convertToBlob({ type: "image/png" });
            const path = await native.saveScreenshot(new Uint8Array(await blob.arrayBuffer()));
            let copied = true;
            await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]).catch(() => (copied = false));
            alert(alerts.success(t(copied ? "native.screenshotCopied" : "native.screenshotSaved", { file: fileName(path) })));
        } catch (error) {
            alert(alerts.error(t("native.screenshotFailed", { detail: errorText(error) })));
        }
    };

    // Atalhos: F9 = print, F10 = gravar/parar.
    const actions = useRef({ screenshot, toggleRecording });
    actions.current = { screenshot, toggleRecording };
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.repeat) return;
            if (event.key === "F9") {
                event.preventDefault();
                actions.current.screenshot();
            } else if (event.key === "F10") {
                event.preventDefault();
                actions.current.toggleRecording();
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    return (
        <NativeContext.Provider
            value={{
                config,
                updateConfig,
                devices,
                status,
                server,
                recording,
                streaming,
                toggleRecording,
                outputDialogOpen,
                openOutputDialog,
                closeOutputDialog,
                startOutputs,
                applyStreamQuality,
                stopRecording,
                streamDirty: streaming === "active" && !!config && streamSnapshot !== null && streamKey(config, outputSource) !== streamSnapshot,
                outputSource,
                p2pLink: config && p2pConfigured(config) && p2pName && "name" in p2pName ? p2pLink(config.stream.p2pServer, p2pName.name) : null,
                p2pLinkError: p2pName && "error" in p2pName ? p2pName.error : null,
                p2pViewers,
                p2pState,
                outputAutoAudioId: outputPaired === null ? (vsctx?.pairedAudioId ?? null) : outputPaired || null,
                captureSources,
                refreshCaptureSources,
                outputs,
                feedback,
                setFeedback,
                outputTrack,
                stopStreaming,
                screenshot,
                settingsTab,
                openSettings: (tab = "audio") => setSettingsTab(tab),
            }}>
            {children}
        </NativeContext.Provider>
    );
}
