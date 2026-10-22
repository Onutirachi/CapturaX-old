// Partes do P2P (WebRTC) usadas pelo app e pela página de quem assiste.
//
// O app guarda um segredo (`stream.p2pKey`); o servidor (`relay/`) prende o nome do link (apelido ou
// código gerado) a ele. Quem só tem o link não consegue se passar por quem transmite.

/** STUN gratuitos: cada lado descobre o próprio endereço público. Sem TURN: o vídeo nunca passa por servidor. */
export const ICE_SERVERS: RTCIceServer[] = [{ urls: ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"] }];

const hex = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

/** Segredo novo (o link muda junto). */
export const newP2PKey = () => hex(crypto.getRandomValues(new Uint8Array(24)));

export const p2pLink = (server: string, name: string) => `https://${server}/${name}`;
export const signalUrl = (server: string) => `wss://${server}/signal`;

/** Nome do link no endereço da página (`/<nome>`), ou `null` fora do modo P2P. */
export function roomFromPath(path: string) {
    return path.match(/^\/([a-z0-9-]{3,20})\/?$/)?.[1] ?? null;
}

/** Apelido aceito pelo servidor: 3 a 20 letras minúsculas, números ou hífen (não nas pontas). */
export const validNickname = (nickname: string) => /^[a-z0-9](?:[a-z0-9-]{1,18})[a-z0-9]$/.test(nickname);

export type ClaimError = "taken" | "invalid" | "unauthorized" | "offline";

/** Reserva (ou renova) o nome do link deste app; vazio = código gerado pelo servidor. */
export async function claimName(server: string, password: string, key: string, nickname: string): Promise<{ name: string } | { error: ClaimError }> {
    try {
        const response = await fetch(`https://${server}/api/claim`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ password, key, nickname: nickname || null }),
        });
        const body = (await response.json()) as { name?: string; error?: string };
        if (body.name) return { name: body.name };
        return { error: body.error === "taken" ? "taken" : body.error === "unauthorized" ? "unauthorized" : "invalid" };
    } catch {
        return { error: "offline" };
    }
}

export type ServerRole = "admin" | "usage" | null;

async function api<T>(server: string, path: string, body: object): Promise<T> {
    const response = await fetch(`https://${server}${path}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    if (!response.ok && response.status !== 401) throw new Error(`HTTP ${response.status}`);
    return response.json() as Promise<T>;
}

/** Que senha é esta nesse servidor (admin, uso ou nenhuma). Lança se o servidor não responder. */
export const serverRole = (server: string, password: string) => api<{ role: ServerRole; usageSet: boolean }>(server, "/api/role", { password });

/** Troca a senha de uso (só com a de admin). Vazia = só o admin transmite. */
export async function setUsagePassword(server: string, admin: string, password: string) {
    const result = await api<{ ok?: boolean; error?: string }>(server, "/api/usage-password", { admin, password });
    if (!result.ok) throw new Error(result.error ?? "failed");
}

/** Mensagens trocadas pela sinalização (ver `relay/src/main.rs`). */
export type SignalData = { sdp?: RTCSessionDescriptionInit; candidate?: RTCIceCandidateInit };
