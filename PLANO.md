# Plano — CapturaX híbrido (Preact + Rust)

Plano combinado em 2026-10-08 (revisado no mesmo dia). Contexto e histórico em [NOTAS.md](NOTAS.md);
arquitetura atual em [AGENTS.md](AGENTS.md).

**Status:** fases 0 e 2–6 implementadas na branch `feat/hibrido` (o que foi validado e o que falta
testar está no NOTAS.md). Fase 1 implementada, mas as medições dependem de rodar com a placa.
Fase 7 continua para o futuro.

## Objetivo

**Prioridade nº 1: usar a placa de captura com qualidade máxima e latência mínima, a ponto de
jogar um console pelo preview sem incômodo.** Tudo o que vier depois (gravação, transmissão,
print) não pode piorar o preview.

A interface continua em Preact, com o visual atual, e vai para o Rust tudo o que for pesado ou
que o navegador não consegue fazer.

## Divisão de responsabilidades

| Fica no WebView (Preact) | Vai para o Rust |
|---|---|
| Interface inteira | **Todo o áudio:** preview de baixa latência, gravação, áudio do sistema, exclusão de processo, mixer, AAC |
| Preview de vídeo da placa (`getUserMedia` + `<video>`) | Muxer MP4 fragmentado (gravação e transmissão usam o mesmo) |
| Codificação de vídeo via **WebCodecs** `VideoEncoder` em um Worker, com `prefer-hardware` (NVENC/QSV/AMF) | Gravação em disco |
| Captura de tela via `getDisplayMedia` | Servidor da transmissão, UPnP e firewall |
| Gerar o PNG do print | Salvar o print e copiar para a área de transferência |

### Por que o vídeo continua sendo capturado no WebView

- A maioria das placas só aceita **um** programa por vez. Se o Rust abrir a placa para gravar,
  o preview perde o acesso.
- No WebView o frame vai da placa para a GPU sem cópia e é comprimido na própria GPU. Para o
  Rust só vão **pacotes comprimidos** (ex. 20 Mbps ≈ 2,5 MB/s); mandar frames crus seria inviável
  (4K60 ≈ 1,5 GB/s).
- Os encoders rodam em um **Worker**, fora da thread da interface, e usam o chip dedicado de
  codificação da GPU. Assim a gravação não deveria atrasar o preview (validar na Fase 1).
- Se a medição mostrar que o preview do WebView tem latência demais para jogar, troca-se só a
  fonte de vídeo por uma nativa (Media Foundation + Direct3D no Rust). Por isso a fonte de vídeo
  fica atrás de uma interface no front (`VideoSource`: placa ou tela).

### Áudio: preview e gravação separados

O usuário escolhe dois caminhos independentes:

1. **Preview (ouvir):** dispositivo de entrada (ex. áudio da placa) → saída escolhida (fone,
   caixa). Passa a ser tocado pelo **Rust via WASAPI de baixa latência** (`IAudioClient3`,
   ~10 ms), no lugar do `<video>` do WebView, que costuma ficar entre 30 e 60 ms. Volume e mudo
   continuam na Dock. Opção de atrasar o áudio em alguns ms se o vídeo ficar para trás (lip-sync).
2. **Gravação/transmissão:** dispositivo escolhido (ex. áudio da placa e/ou microfone) **+ áudio
   do sistema**, com exclusão de processo.

**Como o áudio do sistema é capturado.** O loopback comum do Windows pega tudo o que toca, e isso
inclui o próprio preview do CapturaX. Com a placa gravada direto, o som dela entraria duas vezes
(com eco). A API de exclusão aceita só um processo, e essa vaga é do Discord. Solução:

- Capturar o sistema **app por app** (process loopback em modo *include*, um stream por app que
  esteja tocando som) e mixar todos, **menos** o CapturaX (sempre, automático) e o processo
  escolhido pelo usuário.
- Novos apps que começam a tocar são detectados (`IAudioSessionNotification`) e entram no mix.
- **Sem limite de apps excluídos.** Tela "Áudio dos apps" com checkbox por app:
  - lista os apps tocando som agora e os que já foram vistos antes (com ícone e nome do `.exe`);
  - o CapturaX aparece fixo como excluído (desabilitado, com explicação);
  - botão para adicionar um `.exe` à mão, mesmo que não esteja aberto;
  - a escolha é **salva pelo nome do executável** (ex. `Discord.exe`) no arquivo de configuração
    do Rust, então continua valendo depois de reiniciar o app ou o Discord;
  - pode ser alterada durante uma gravação/transmissão, e a mudança vale na hora.
- Requer Windows 10 2004+.

### Fluxo de dados

```
Placa (vídeo) ──► WebView: <video> (preview)
Tela (vídeo) ──┘     └──► Worker: VideoEncoder (GPU) ──► H.264 ──IPC──┐
                                                                     ▼
Placa (áudio) ──► Rust: preview WASAPI ──► fone/caixa       Rust: muxer fMP4 ──► .mp4 (gravação)
      └───────────────────────┐                                ▲       └──► HLS ──► servidor ──► link
Microfone ────────────────────┼──► Rust: mixer ──► AAC ────────┘
Apps (menos CapturaX e os ───┘
 apps desmarcados)
```

## Visual

- **Laranja `#FF7304` (da logo) vira a cor de destaque:** estados ativos, sliders, foco, logo.
- **Azul vira cor secundária/informativa:** alertas de informação, links, tags.
- **Vermelho continua para erro e para o indicador de gravação (REC).**
- Ajustar os tons do laranja no tema escuro para manter contraste nos textos.

## Fases

### Fase 0 — Logo e cores (rápida)
- `X.svg` → ícone do app (`npm run tauri icon`); `CapturaX.svg` → WindowBar e splash do
  `PermissionGate`.
- Limpar os SVGs do CorelDRAW (tirar DOCTYPE e metadata, manter o `viewBox`) e mover para
  `public/assets/`.
- Novos tokens em `Globals.css` (`--color-accent` laranja, azul como secundária) e aplicar nos
  componentes.
- `tauri.conf.json`: `productName` "CapturaX".

### Fase 1 — Fundação e medições (define o resto)
- Estrutura em `src-tauri/src/`: `audio/`, `media/` (mux, recorder), `stream/` (server, rede),
  `commands.rs`. Runtime `tokio`; pacotes binários do front para o Rust via `tauri::ipc` (sem
  JSON); eventos do Rust para o front.
- **Medições antes de seguir:**
  1. **Latência do preview** (o mais importante): a câmera do celular filmando a TV e o monitor
     lado a lado em câmera lenta, comparando com o OBS. Se o WebView perder por mais de ~1 frame,
     priorizar o preview nativo antes da Fase 4.
  2. `VideoEncoder.isConfigSupported` com `prefer-hardware` em 1080p60 e 4K60 (H.264, HEVC, AV1)
     no WebView2 desta máquina. Medir CPU/GPU e **confirmar que gravar não aumenta a latência do
     preview**.
  3. Relógios: alinhar o `timestamp` dos `VideoFrame` com o QPC do áudio WASAPI. Gravar 30 min e
     checar se áudio e vídeo continuam sincronizados.
- Atualizar o AGENTS.md (hoje diz "no custom Rust commands", mas já existe `set_window_animations`).

### Fase 2 — Print
- O frame atual em resolução nativa vai para um canvas e vira PNG; o Rust salva em
  `Imagens/CapturaX/` e copia para a área de transferência.
- Botão na Dock + atalho de teclado; alerta com o caminho salvo.

### Fase 3 — Motor de áudio no Rust
- WASAPI (crate `windows`): listar dispositivos de entrada e saída.
- Preview de baixa latência (entrada → saída escolhida), volume/mudo/atraso; o `<video>` fica
  mudo.
- Captura app por app com exclusão (ver seção de áudio acima), microfone e áudio da placa.
- Tela "Áudio dos apps" com checkboxes e escolha salva.
- Mixer com volume por fonte; codificador AAC do Media Foundation (já vem no Windows).

### Fase 4 — Gravação
- Muxer fMP4 próprio no Rust (vídeo + AAC). O MP4 fragmentado não corrompe se o app fechar no
  meio. Sem FFmpeg (evita ~100 MB e questões de licença).
- **Qualidade escolhível:** presets (Original, Alta, Média, Leve) + avançado: resolução, FPS,
  bitrate e codec (H.264 sempre; HEVC/AV1 só se a GPU suportar). Padrão: igual ao preview.
- Fontes de áudio da gravação, pasta de destino.
- Na interface: botão gravar/parar, tempo decorrido e tamanho do arquivo na StatusBar.

### Fase 5 — Captura de tela
- Nova `VideoSource` "Tela" (`getDisplayMedia`, monitor ou janela) com o mesmo preview, print,
  gravação e transmissão.

### Fase 6 — Transmissão por link

**Formato.** HLS: segmentos fMP4 de 1–2 s, atraso de ~4–8 s. Os segmentos são gerados uma vez
e só servidos por HTTP; quem assiste abre em qualquer navegador.

**Capacidade.** 10 pessoas é tranquilo para o computador. O limite é o **upload** da internet:
bitrate × pessoas (ex. 720p30 a 3 Mbps × 10 = 30 Mbps; 1080p60 a 8 Mbps × 10 = 80 Mbps). A
configuração mostra essa conta e tem limite de espectadores.

**Qualidade.** Resolução, FPS e bitrate escolhidos por quem transmite, em um encoder separado
do da gravação (frame reduzido na GPU, num Worker).

**Link fixo.** `http://<ip-ou-domínio>:<porta>/`, sem token e sem senha.
- Com a transmissão parada, a página mostra "Offline" e **entra sozinha** quando a transmissão
  começa, sem o amigo precisar recarregar.
- O servidor sobe junto com o app se a opção "aceitar espectadores" estiver ligada (padrão
  desligado).
- O IP público da maioria das conexões residenciais muda de tempos em tempos. Para o link
  continuar o mesmo, a configuração sugere um DDNS gratuito (DuckDNS, No-IP, ou o do próprio
  roteador) e mostra o link atual com botão de copiar.

**Página de quem assiste.** É uma segunda entrada do Vite (`viewer.html`), reaproveitando
componentes do app (AmbientLight, slider de volume, botões), e fica embutida no executável. Tem:
- liga/desliga do **ambient light**, que preenche as bordas quando a proporção não bate (ex.
  quem transmite usa 21:9 e quem assiste tem tela 16:9);
- **volume/mudo**;
- **tela cheia**.

Esses controles mudam só o navegador de quem assiste; nada volta para o PC de quem transmite.

**Abrir a porta automaticamente.**
- **Roteador: UPnP IGD** (crate `igd-next`), com NAT-PMP/PCP como alternativa. Ao ligar a
  transmissão, o app pede ao roteador para redirecionar a porta, renova o pedido periodicamente
  e remove ao desligar. Funciona na maioria dos roteadores domésticos com UPnP ativado; se não
  funcionar, a interface explica como abrir a porta na mão.
- **Firewall do Windows:** o instalador (NSIS do Tauri, que já roda como administrador) cria uma
  regra liberando **só o executável do CapturaX, só a porta TCP escolhida**. Se a porta mudar
  depois, um botão "corrigir firewall" pede elevação (UAC) e refaz a regra.
- **CGNAT:** muitas operadoras (comum no Brasil) colocam o cliente atrás de CGNAT, e aí nenhuma
  porta aberta no roteador chega da internet. O app detecta (IP externo informado pelo roteador ≠
  IP público) e avisa. Alternativas: pedir IP público à operadora, IPv6, ou um túnel (Cloudflare
  Tunnel / Tailscale Funnel). O `StreamSink` (abaixo) permite plugar um relay depois.

**Segurança contra invasão/exploit.** O vídeo ser público não é problema; o foco é ninguém
conseguir mexer no PC.
- Servidor em Rust (`axum`/`hyper`), sem código inseguro no caminho da rede.
- **Somente leitura e só da memória:** responde apenas GET em rotas fixas (página, playlist,
  segmentos). Não lê arquivos do disco a partir da URL (sem risco de `../`), não aceita upload e
  não tem nenhuma rota que controle o app.
- Limites contra abuso: tamanho máximo de requisição/cabeçalhos, timeouts (contra conexões
  lentas de propósito), máximo de espectadores e de conexões por IP, rate limit.
- Cabeçalhos seguros (CSP restritiva, `nosniff`, sem `Server` com versão).
- O app roda como usuário comum; só a criação da regra de firewall usa administrador.
- Lista de quem está assistindo (IP) na interface, com opção de bloquear.
- Dependências de rede auditadas (`cargo audit`) antes de cada versão.

**Servidor externo depois.** A saída de transmissão fica atrás de uma interface (`StreamSink`).
Hoje: `LocalHlsSink` (servidor no PC). Depois: `RelaySink`, que envia os mesmos segmentos por
HTTPS para um servidor hospedado. O app e a página de quem assiste não mudam.

### Fase 9 — Próximos (combinados em 2026-10-08, ainda não implementados)
1. ~~**Áudio automático pareado com o vídeo**~~ — **feito** (2026-10-09). "Auto" no Audio Device
   (padrão): o Rust lê o `ContainerId` do vídeo (Media Foundation → devnode) e das entradas WASAPI;
   o WebView informa o par do vídeo atual (`audio_set_paired_input`) e o Rust resolve na hora.
   Tela/janela ou aparelho sem áudio = sem áudio. Substituiu o palpite pelo nome.
2. ~~**Telas e janelas no dropdown de vídeo, sem o seletor do navegador**~~ — **feito**
   (2026-10-09). Windows Graphics Capture no Rust (sem borda amarela, com cursor), grupos
   "Devices / Screens / Windows" no dropdown, telas como "Screen 2 - 27G2G4". Em vez de levar a
   codificação para o Rust, os quadros vão ao WebView por memória compartilhada do WebView2 e viram
   uma trilha de vídeo comum: preview completo e o mesmo caminho de gravação/transmissão/print.
   Custo medido: 1080p a ~90 fps ≈ 2% da CPU (build de debug). Se um dia pesar (4K a 144 Hz),
   converter para NV12 na GPU antes de copiar reduz a cópia pela metade.
3. ~~**Critério do automático / priorizar fluidez**~~ — **feito** (2026-10-09). O Auto continua
   "o mais fluido até ~60 fps, depois a maior resolução" (placa: 1440p60; webcam: 1080p30). Opção
   extra **Smoothest** no Mode: o maior FPS a partir de 720p (ezcap: 1080p120); só aparece quando
   é diferente do Auto.

### Fase 11 — Fontes da gravação/transmissão — **feito** (2026-10-09)
Decidido com o usuário: o preview (barra de cima) é só para dispositivos — ver a própria tela no
CapturaX não faz sentido. Gravação e transmissão escolhem a fonte nas suas janelas: a mesma do
preview, uma tela ou uma janela (ex.: transmitir a tela inteira com o CapturaX mostrando o PS5).
Botões "Ver gravação" / "Ver live" mostram no preview o que está saindo e travam os dropdowns até
voltar (apagados quando a fonte é a do preview). Placas aceitam um dono só, então a saída nunca abre
outro dispositivo. Áudio separado: o que você ouve × o que vai junto; o "Auto" do áudio gravado é o
do aparelho do preview (o som do console vai junto ao transmitir a tela). Saída de áudio só nas
configurações. Barra de baixo mostra fonte, resolução e FPS de cada saída.

### Fase 10 — Assistir sem abrir porta — **P2P feito** (2026-10-22)
Escolhido o P2P (WebRTC, como o vdo.ninja): a VPS Oracle do usuário (`capturax.duckdns.org`) só
hospeda a página e a sinalização (`relay/`); o vídeo vai direto do PC para cada espectador (STUN
grátis, sem TURN). Testado de ponta a ponta na mesma máquina; falta testar de fora (4G). Se alguns
espectadores não conectarem (NAT restritivo dos dois lados), adicionar coturn na VPS. Abaixo, o
estudo original.

### Fase 10 (estudo original)
Hoje quem assiste pela internet precisa da porta aberta no roteador (UPnP ou manual) e não
funciona atrás de CGNAT. Opções gratuitas levantadas em 2026-10-09, para entender antes de decidir:
- **IPv6:** sem NAT, não precisa abrir porta no roteador (só o firewall). Quem assiste também
  precisa de IPv6 (celular quase sempre tem; alguns Wi-Fi não). Alguns roteadores bloqueiam
  entrada em IPv6. Barato: mostrar um link IPv6 quando houver.
- **WebRTC P2P:** o vídeo vai direto do PC para quem assiste, furando o NAT sozinho (STUN gratuito).
  Só a "apresentação" inicial passa por um servidor pequeno e gratuito (ex.: Cloudflare Workers),
  sem vídeo passando por ele. Funciona na grande maioria das redes, link fixo, menor atraso
  (~0,2 s). Não funciona quando os dois lados estão em NAT restritivo (precisaria de um servidor de
  repasse TURN, que custa banda). Mudança grande, mas o WebView já tem WebRTC com encoder de GPU.
- **Cloudflare Tunnel:** o app abre uma conexão de saída e a Cloudflare repassa. O "quick tunnel"
  dispensa conta, mas o link muda a cada vez; link fixo exige domínio próprio. Os termos do plano
  grátis restringem servir vídeo — arriscado para uso contínuo.
- **Tailscale Funnel:** link fixo `*.ts.net`, grátis, mas exige instalar o Tailscale e criar conta;
  banda limitada por eles.
- **ngrok grátis:** 1 GB/mês, a live gasta isso em ~20 min. Descartado.

### Fase 8 — Multi-idioma — **feito** (2026-10-09)
- **Idiomas iniciais:** inglês e português (Brasil). Inglês é a base e o fallback.
- **Detecção automática:** o Rust informa o idioma da interface do Windows
  (`GetUserDefaultUILanguage`, comando `system_locale`). Procura `pt-BR` exato → idioma `pt` →
  inglês. A página de quem assiste usa o idioma do navegador de quem assiste
  (`navigator.languages`), independente do app.
- **Escolha manual:** Configurações → Geral → Idioma ("Automático (sistema)" + lista), salvo no
  `settings.json` (`language`). Troca na hora, sem reiniciar.
- Feito com `GetUserPreferredUILanguages` (comando `system_languages`), `src/i18n/index.tsx`
  (`t`, `tx` para texto com elementos, `useT`, `errorText`) e erros do Rust em `src-tauri/src/error.rs`.
- **Estrutura para crescer:**
  - Um arquivo por idioma em `src/i18n/` (`en.ts` é a fonte das chaves; os outros são tipados como
    `Record<keyof typeof en, string>`, então faltar uma tradução é erro de compilação).
  - Função `t("chave", { variáveis })` própria (sem dependência pesada), plurais com
    `Intl.PluralRules`, números/datas/tamanhos com `Intl`.
  - Adicionar um idioma = criar o arquivo e registrar numa lista; aparece sozinho no seletor.
- **Mensagens do Rust:** os erros passam a voltar como código + dados (ex.: `port_unavailable`,
  `{ port }`) e o front traduz; nada de texto em inglês montado no Rust aparecendo na tela.
- Os textos atuais da interface (hoje em inglês) viram as chaves do `en.ts`.

### Fase 7 — Futuro
- ~~Modos exatos do dispositivo (Media Foundation) no lugar da lista fixa de `constants.ts`~~ —
  **feito** (dropdown único "Mode" com os modos reais da placa).
- Preview de vídeo nativo, se a medição da Fase 1 justificar (pode ser antecipado).
- Servidor relay hospedado.

## Decisões tomadas
- Cores: laranja de destaque, azul secundário, vermelho para erro/REC.
- Áudio de preview e de gravação escolhidos separadamente; sistema capturado app por app, sem o
  CapturaX e sem quantos apps o usuário desmarcar (escolha salva).
- Link fixo, sem token/senha, com tela "Offline"; segurança focada em não expor o PC.
- Porta aberta automaticamente via UPnP + regra de firewall criada pelo instalador.
