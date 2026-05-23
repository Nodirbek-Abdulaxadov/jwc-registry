# --- builder ---
FROM rust:1.83-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release --bin jwc-registry

# --- runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/jwc-registry /usr/local/bin/jwc-registry
ENV REGISTRY_BIND=0.0.0.0:8080 \
    REGISTRY_STORAGE_PATH=/var/lib/jwc-registry/storage
RUN mkdir -p /var/lib/jwc-registry/storage
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s \
    CMD wget -q -O- http://127.0.0.1:8080/healthz || exit 1
CMD ["jwc-registry"]
