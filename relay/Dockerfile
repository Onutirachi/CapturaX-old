# Imagem do servidor P2P do CapturaX: sinalização + página de quem assiste.
# Contexto = raiz do repositório (a página reaproveita componentes de src/).
#   docker build -f relay/Dockerfile -t capturax-signal .

# 1. Página de quem assiste (viewer/ → src-tauri/viewer-dist)
FROM node:22-slim AS viewer
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY vite.viewer.config.ts tsconfig.json tsconfig.node.json ./
COPY viewer viewer
COPY src src
COPY public public
RUN npm run build:viewer

# 2. Servidor de sinalização
FROM rust:1-slim AS server
WORKDIR /relay
COPY relay/Cargo.toml relay/Cargo.lock ./
COPY relay/src src
RUN cargo build --release --locked

# 3. Imagem final: só o binário e a página, rodando sem root
FROM debian:bookworm-slim
COPY --from=server /relay/target/release/capturax-signal /usr/local/bin/capturax-signal
COPY --from=viewer /app/src-tauri/viewer-dist /site
RUN mkdir /data && chown 65532:65532 /data
USER 65532:65532
ENV CAPTURAX_SIGNAL_ADDR=0.0.0.0:8790 CAPTURAX_DATA=/data CAPTURAX_SITE=/site
VOLUME /data
EXPOSE 8790
CMD ["capturax-signal"]
