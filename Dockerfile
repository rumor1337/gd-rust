# gdrust live streamer: Rust render loop + ffmpeg + Node gift bridge.
# Build context = repo root (levels/, audio/, macros/ are not in git; deploy.sh rsyncs them).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin live

FROM node:20-bookworm-slim AS bridge
WORKDIR /bridge
COPY bridge/package.json bridge/package-lock.json ./
RUN npm ci --omit=dev

FROM node:20-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/live /usr/local/bin/live
COPY levels ./levels
COPY data ./data
COPY macros ./macros
COPY audio ./audio
COPY bridge/index.js ./bridge/index.js
COPY --from=bridge /bridge/node_modules ./bridge/node_modules
ENV LIVE_ROOT=/app LIVE_ASSETS=/assets
USER node
CMD ["live"]
