// Partes do P2P (WebRTC) usadas pelo app e pela página de quem assiste.
//
// O app guarda um segredo (`stream.p2pKey`); a sala do link é `sha256(segredo)` (40 hex), igual ao
// servidor de sinalização (`relay/`). Quem só tem o link não consegue se passar por quem transmite.

/** STUN gratuitos: cada lado descobre o próprio endereço público. Sem TURN: o vídeo nunca passa por servidor. */
export const ICE_SERVERS: RTCIceServer[] = [{ urls: ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"] }];

const hex = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

export async function roomOf(key: string) {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key));
    return hex(new Uint8Array(digest).slice(0, 20));
}

/** Segredo novo (o link muda junto). */
export const newP2PKey = () => hex(crypto.getRandomValues(new Uint8Array(24)));

export const p2pLink = (server: string, room: string) => `https://${server}/v/${room}`;
export const signalUrl = (server: string) => `wss://${server}/signal`;

/** Sala no endereço da página (`/v/<sala>`), ou `null` fora do modo P2P. */
export function roomFromPath(path: string) {
    return path.match(/^\/v\/([0-9a-f]{40})\/?$/)?.[1] ?? null;
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
