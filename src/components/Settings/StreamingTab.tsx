import { useContext, useState } from "preact/hooks";
import { NativeContext } from "../../context/NativeContext";
import { native, errorText, type StreamConfig, type UpnpStatus } from "../../core/native/api";
import { Badge, Button, CopyField, Note, NumberInput, Row, Section, Toggle } from "./controls";
import { t as translate, tx, useT } from "../../i18n";
import P2PSettings from "./P2PSettings";

function formatWatching(seconds: number) {
    if (seconds < 60) return translate("time.seconds", { n: seconds });
    const minutes = Math.floor(seconds / 60);
    return minutes < 60 ? translate("time.minutes", { n: minutes }) : translate("time.hoursMinutes", { h: Math.floor(minutes / 60), m: minutes % 60 });
}

function UpnpText({ status }: { status: UpnpStatus }) {
    const t = useT();
    switch (status.state) {
        case "disabled":
            return <>{t("upnp.disabled")}</>;
        case "searching":
            return <>{t("upnp.searching")}</>;
        case "mapped":
            return status.cgnat ? (
                <span className="text-[goldenrod]">{t("upnp.cgnat", { ip: status.externalIp })}</span>
            ) : (
                <>{t("upnp.mapped", { ip: status.externalIp })}</>
            );
        case "failed":
            return <span className="text-[goldenrod]">{t("upnp.failed", { reason: errorText(status.reason) })}</span>;
    }
}

export default function StreamingTab() {
    const t = useT();
    const nctx = useContext(NativeContext)!;
    const stream = nctx.config!.stream;
    const server = nctx.server;
    const set = (patch: Partial<StreamConfig>) => nctx.updateConfig((c) => ({ ...c, stream: { ...c.stream, ...patch } }));
    const [firewallError, setFirewallError] = useState<string | null>(null);
    const [fixing, setFixing] = useState(false);

    const live = nctx.streaming === "active";
    const cgnat = server?.upnp.state === "mapped" && server.upnp.cgnat;

    const fixFirewall = async () => {
        setFixing(true);
        setFirewallError(null);
        await native.firewallFix().catch((e) => setFirewallError(errorText(e)));
        setFixing(false);
    };

    return (
        <div className="flex flex-col gap-6">
            <Section title={t("streaming.link")} description={t("streaming.linkDescription")}>
                <Row label={t("streaming.acceptViewers")} hint={t("streaming.acceptViewersHint")}>
                    <Toggle checked={stream.acceptViewers} onChange={(acceptViewers) => set({ acceptViewers })} />
                </Row>
                <Row label={t(live ? "streaming.live" : "streaming.notLive")} hint={live ? t("streaming.watchingNow", { count: (server?.viewers.length ?? 0) + nctx.p2pViewers }) : t("streaming.notLiveHint")}>
                    {live && <Button onClick={() => nctx.openOutputDialog()}>{t("streaming.changeQuality")}</Button>}
                    <Button variant={live ? "danger" : "accent"} onClick={() => (live ? nctx.stopStreaming() : nctx.openOutputDialog())} disabled={nctx.streaming === "starting" || nctx.streaming === "stopping"}>
                        {t(live ? "streaming.stop" : "streaming.goLive")}
                    </Button>
                </Row>
                {server?.publicUrl && (
                    <Row label={t("streaming.internetLink")} hint={t("streaming.internetLinkHint")}>
                        <CopyField value={server.publicUrl} />
                    </Row>
                )}
                {server?.publicUrlV6 && (
                    <Row label={t("streaming.internetLinkV6")} hint={t("golive.ipv6Hint")}>
                        <CopyField value={server.publicUrlV6} />
                    </Row>
                )}
                {server?.running && !server.publicUrlV6 && <Row label={t("streaming.internetLinkV6")} hint={t("streaming.noIpv6")} />}
                {server?.localUrl && (
                    <Row label={t("streaming.localLink")} hint={t("streaming.localLinkHint")}>
                        <CopyField value={server.localUrl} />
                    </Row>
                )}
                {server?.error && (
                    <div className="px-4 py-3">
                        <Note tone="error">{errorText(server.error)}</Note>
                    </div>
                )}
            </Section>

            <P2PSettings />

            <Section title={t("streaming.network")}>
                <Row label={t("streaming.port")} hint={t("streaming.portHint")}>
                    <NumberInput value={stream.port} min={1024} max={65535} onChange={(port) => set({ port })} />
                </Row>
                <Row label={t("streaming.upnp")} hint={server ? <UpnpText status={server.upnp} /> : t("streaming.upnpOffline")}>
                    <Toggle checked={stream.upnp} onChange={(upnp) => set({ upnp })} />
                </Row>
                <Row
                    label={t("streaming.firewall")}
                    hint={
                        server?.firewallOk ? (
                            t("streaming.firewallOk")
                        ) : (
                            <>{t("streaming.firewallMissing")}{firewallError && <span className="text-bad"> {errorText(firewallError)}</span>}</>
                        )
                    }>
                    {server?.firewallOk ? <Badge color="var(--color-accent)">OK</Badge> : <Button onClick={fixFirewall} disabled={fixing}>{t(fixing ? "streaming.waiting" : "streaming.fixFirewall")}</Button>}
                </Row>
            </Section>

            <Section title={t("streaming.viewers")} description={t("streaming.viewersDescription")}>
                {(server?.viewers ?? []).length === 0 && <Row label={<span className="text-(--text-muted)">{t("streaming.nobody")}</span>} />}
                {(server?.viewers ?? []).map((viewer) => (
                    <Row key={viewer.ip} label={viewer.ip} hint={t("streaming.watchingFor", { time: formatWatching(viewer.watchingSecs) })}>
                        <Button variant="danger" onClick={() => set({ blockedIps: [...stream.blockedIps, viewer.ip] })}>
                            {t("streaming.block")}
                        </Button>
                    </Row>
                ))}
                {stream.blockedIps.map((ip) => (
                    <Row key={ip} label={<span className="text-(--text-muted)">{ip}</span>} hint={t("streaming.blocked")}>
                        <Button onClick={() => set({ blockedIps: stream.blockedIps.filter((b) => b !== ip) })}>{t("streaming.unblock")}</Button>
                    </Row>
                ))}
            </Section>

            <Note tone={cgnat ? "warning" : "info"}>
                <p>
                    {tx("streaming.ddns", { title: <b>{t("streaming.ddnsTitle")}</b>, example: <code>myname.duckdns.org:{stream.port}</code> })}
                </p>
                <p className="mt-2">
                    {tx("streaming.tunnel", { title: <b>{t("streaming.tunnelTitle")}</b>, address: <code>http://localhost:{stream.port}</code> })}
                </p>
            </Note>
        </div>
    );
}
