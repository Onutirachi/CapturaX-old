// Servidor P2P (ver relay/README.md): endereço + senha (de uso ou de admin). Com a de admin,
// aparece o campo para trocar a senha de uso que o dono passa para os amigos.
import { useContext, useEffect, useState } from "preact/hooks";
import { NativeContext } from "../../context/NativeContext";
import { newP2PKey, serverRole, setUsagePassword, type ServerRole } from "../../core/p2p/common";
import { useT } from "../../i18n";
import { Button, CopyField, NumberInput, Row, Section } from "./controls";

type Check = { state: "empty" } | { state: "checking" } | { state: "offline" } | { state: "checked"; role: ServerRole; usageSet: boolean };

const input = "w-64 h-9 px-3 rounded-lg bg-dark [border:var(--border)] outline-none focus:border-accent";
const cleanServer = (value: string) => value.trim().replace(/^(https?|wss?):\/\//, "").replace(/\/.*$/, "");

export default function P2PSettings() {
    const t = useT();
    const nctx = useContext(NativeContext)!;
    const stream = nctx.config!.stream;
    const set = (patch: Partial<typeof stream>) => nctx.updateConfig((c) => ({ ...c, stream: { ...c.stream, ...patch } }));
    const [check, setCheck] = useState<Check>({ state: "empty" });
    const [usage, setUsage] = useState("");
    const [usageSaved, setUsageSaved] = useState<"idle" | "saving" | "saved" | "failed">("idle");

    // Confere a senha no servidor (com uma pausa, para não perguntar a cada tecla).
    useEffect(() => {
        if (!stream.p2pServer || !stream.p2pPassword) return setCheck({ state: "empty" });
        setCheck({ state: "checking" });
        let current = true;
        const timer = window.setTimeout(() => {
            serverRole(stream.p2pServer, stream.p2pPassword)
                .then(({ role, usageSet }) => current && setCheck({ state: "checked", role, usageSet }))
                .catch(() => current && setCheck({ state: "offline" }));
        }, 600);
        return () => {
            current = false;
            window.clearTimeout(timer);
        };
    }, [stream.p2pServer, stream.p2pPassword]);

    const saveUsage = async () => {
        setUsageSaved("saving");
        try {
            await setUsagePassword(stream.p2pServer, stream.p2pPassword, usage);
            setUsageSaved("saved");
            setUsage("");
            setCheck((current) => (current.state === "checked" ? { ...current, usageSet: usage !== "" } : current));
        } catch {
            setUsageSaved("failed");
        }
    };

    const statusText = () => {
        if (check.state === "empty") return t("p2p.statusEmpty");
        if (check.state === "checking") return t("p2p.statusChecking");
        if (check.state === "offline") return t("p2p.statusOffline");
        if (check.role === "admin") return t("p2p.statusAdmin");
        if (check.role === "usage") return t("p2p.statusUsage");
        return t("p2p.statusWrong");
    };
    const status = statusText();
    const ok = check.state === "checked" && check.role !== null;

    return (
        <Section title={t("p2p.title")} description={t("p2p.description")}>
            <Row label={t("p2p.server")} hint={t("p2p.serverHint")}>
                <input value={stream.p2pServer} placeholder="meucapturax.duckdns.org" onChange={(e) => set({ p2pServer: cleanServer(e.currentTarget.value) })} className={input} />
            </Row>
            <Row label={t("p2p.password")} hint={<span className={ok ? "" : check.state === "checked" || check.state === "offline" ? "text-[goldenrod]" : ""}>{status}</span>}>
                <input type="password" value={stream.p2pPassword} autoComplete="off" onChange={(e) => set({ p2pPassword: e.currentTarget.value })} className={input} />
            </Row>
            {check.state === "checked" && check.role === "admin" && (
                <Row
                    label={t("p2p.usagePassword")}
                    hint={usageSaved === "saved" ? t("p2p.usageSaved") : usageSaved === "failed" ? t("p2p.usageFailed") : check.usageSet ? t("p2p.usageHint") : t("p2p.usageNone")}>
                    <input type="text" value={usage} autoComplete="off" placeholder={t("p2p.usagePlaceholder")} onInput={(e) => (setUsage(e.currentTarget.value), setUsageSaved("idle"))} className={input} />
                    <Button onClick={saveUsage} disabled={usageSaved === "saving"}>
                        {t("p2p.usageSave")}
                    </Button>
                </Row>
            )}
            {nctx.p2pLink && (
                <Row label={t("p2p.label")} hint={t("p2p.linkHint")}>
                    <CopyField value={nctx.p2pLink} />
                    <Button onClick={() => set({ p2pKey: newP2PKey() })}>{t("p2p.newLink")}</Button>
                </Row>
            )}
            <Row label={t("p2p.maxViewers")} hint={t("p2p.maxViewersHint")}>
                <NumberInput value={stream.p2pMaxViewers} min={1} max={30} onChange={(p2pMaxViewers) => set({ p2pMaxViewers })} />
            </Row>
        </Section>
    );
}
