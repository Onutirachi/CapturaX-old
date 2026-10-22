# Servidor P2P do CapturaX

Deixa quem assiste ver a sua transmissão **sem abrir porta no roteador**. O vídeo vai direto do
seu PC para cada espectador (WebRTC). Este servidor só apresenta os dois lados e entrega a página
de quem assiste, então quase não usa banda nem CPU.

O CapturaX não vem com servidor configurado. Quem quiser o link P2P monta o seu, numa VPS
qualquer, e põe o endereço e a senha no app.

## Senhas

- **Admin:** a sua. Vai no seu CapturaX. Transmite e libera um campo no app para trocar a senha de uso.
- **Uso:** a que você passa para os amigos que podem transmitir pelo seu servidor. Trocar a senha
  de uso desconecta na hora quem estava usando a antiga.
- **Quem só assiste não precisa de senha:** basta o link.

## O que você precisa

- Uma VPS com Linux, Docker e as portas **80** e **443** liberadas. Na Oracle Cloud, libere as
  portas também na *Security List* da rede, além do firewall do sistema.
- Um nome apontando para a VPS, por exemplo um subdomínio grátis do [DuckDNS](https://www.duckdns.org).

## Instalação

```bash
git clone https://github.com/Onutirachi/CapturaX.git /opt/capturax
cd /opt/capturax/relay/deploy

# 1. Proxy com HTTPS automático (uma vez por máquina; serve para outros projetos também)
sudo cp -r proxy /opt/proxy
sudo cp /opt/proxy/sites/capturax.caddy.example /opt/proxy/sites/capturax.caddy
sudo nano /opt/proxy/sites/capturax.caddy        # troque o domínio
(cd /opt/proxy && sudo docker compose up -d)

# 2. Servidor do CapturaX
cp .env.example .env
nano .env                                        # DuckDNS (opcional) e senha de admin (opcional)
sudo docker compose --profile duckdns up -d --build   # sem DuckDNS: tire "--profile duckdns"
sudo docker compose logs capturax-signal              # mostra a senha de admin na primeira vez
```

No CapturaX, em **Configurações → Transmissão → Link sem abrir porta (P2P)**, coloque o endereço
(ex.: `meucapturax.duckdns.org`) e a senha.

## Comandos úteis

Sempre dentro de `/opt/capturax/relay/deploy`:

| Para | Comando |
|---|---|
| Ver se está rodando | `sudo docker compose ps` |
| Ver os logs | `sudo docker compose logs -f capturax-signal` |
| **Perdeu a senha de admin** | `sudo docker compose exec capturax-signal capturax-signal reset-admin` |
| Atualizar | `git pull && sudo docker compose --profile duckdns up -d --build` |
| Parar | `sudo docker compose down` (as senhas continuam salvas) |

## Backup e mudança de VPS

As senhas ficam no volume `capturax_data`, e o certificado HTTPS no volume `proxy_caddy_data`.
Para mudar de máquina, basta instalar de novo e, se quiser manter as senhas, copiar o volume:

```bash
sudo docker run --rm -v capturax_data:/data -v "$PWD":/backup alpine tar czf /backup/capturax-data.tgz -C /data .
# na máquina nova, depois da instalação:
sudo docker run --rm -v capturax_data:/data -v "$PWD":/backup alpine tar xzf /backup/capturax-data.tgz -C /data
```

## Como funciona

- O app conecta em `wss://<domínio>/signal` como quem transmite (com a senha).
- Quem abre `https://<domínio>/v/<sala>` conecta como espectador.
- Os dois trocam por aqui as informações de conexão (alguns KB) e depois o vídeo vai direto entre eles.
- A sala é derivada de um segredo do app (`sha256`), então quem só tem o link não consegue se
  passar por quem transmite.
- **Limitação:** não há servidor de retransmissão (TURN). Em redes muito restritivas dos dois lados
  (algumas operadoras de celular), a conexão pode não fechar.
