import { type ComponentChildren } from "preact";
import { useCallback, useEffect, useState } from "preact/hooks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useT, type MessageKey } from "../../i18n";

type PermissionStatus = "checking" | "granted" | "blocked" | "notFound" | "inUse" | "error";

type Problem = Exclude<PermissionStatus, "checking" | "granted">;
const MESSAGES: Record<Problem, { title: MessageKey; text: MessageKey }> = {
    blocked: { title: "permission.blocked.title", text: "permission.blocked.text" },
    notFound: { title: "permission.notFound.title", text: "permission.notFound.text" },
    inUse: { title: "permission.inUse.title", text: "permission.inUse.text" },
    error: { title: "permission.error.title", text: "permission.error.text" },
};

function statusFromError(error: unknown): PermissionStatus {
    const name = error instanceof DOMException ? error.name : "";
    if (name === "NotAllowedError" || name === "SecurityError") return "blocked";
    if (name === "NotFoundError" || name === "OverconstrainedError") return "notFound";
    if (name === "NotReadableError" || name === "AbortError") return "inUse";
    return "error";
}

async function releaseStream(constraints: MediaStreamConstraints) {
    const stream = await navigator.mediaDevices.getUserMedia(constraints);
    stream.getTracks().forEach((track) => track.stop());
}

export default function PermissionGate({ children }: { children: ComponentChildren }) {
    const [status, setStatus] = useState<PermissionStatus>("checking");
    const t = useT();

    const request = useCallback(async () => {
        try {
            await releaseStream({ video: true });
        } catch (error) {
            const status = statusFromError(error);
            // Sem câmera nenhuma o app ainda grava/transmite telas; câmera ocupada (outro app) não é
            // falta de permissão: segue, e o erro aparece só nesse dispositivo (dá para escolher outro).
            if (status !== "notFound" && status !== "inUse") {
                console.error("Error to get camera permission: ", error);
                setStatus(status);
                return;
            }
        }
        // Microfone é opcional: sem ele o app funciona só com vídeo.
        await releaseStream({ audio: true }).catch((error) => console.warn("Microphone unavailable: ", error));
        setStatus("granted");
    }, []);

    useEffect(() => {
        request();
    }, [request]);

    useEffect(() => {
        if (status === "granted" || status === "checking") return;
        navigator.mediaDevices.addEventListener("devicechange", request);
        return () => navigator.mediaDevices.removeEventListener("devicechange", request);
    }, [status, request]);

    if (status === "granted") return <>{children}</>;
    if (status === "checking") return null;

    const { title, text } = MESSAGES[status];

    return (
        <div className="absolute inset-0 flex items-center justify-center p-5">
            <div className="flex flex-col items-center gap-4 max-w-md p-8 text-center rounded-xl bg-(--background-color-darker)/80 backdrop-blur-xs [border:var(--border)]">
                <img src="./assets/Brand/logo.svg" alt="CapturaX" draggable={false} className="h-8 mb-4 select-none" />
                <div className="w-10 h-10 mask-center mask-contain mask-no-repeat bg-bad mask-[url(/assets/Alerts/error.svg)]" />
                <h1 className="text-xl font-semibold">{t(title)}</h1>
                <p className="text-(--text-muted)">{t(text)}</p>
                <div className="flex gap-3 mt-2">
                    {status === "blocked" && (
                        <button
                            className="px-4 py-2 rounded-lg bg-(--background-color-normal) hover:brightness-125 transition"
                            onClick={() => openUrl("ms-settings:privacy-webcam")}>
                            {t("permission.openSettings")}
                        </button>
                    )}
                    <button className="px-4 py-2 rounded-lg bg-accent hover:brightness-110 transition" onClick={request}>
                        {t("permission.tryAgain")}
                    </button>
                </div>
            </div>
        </div>
    );
}
