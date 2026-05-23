# --- builder ---
# Pinned at 1.90 — the transitive `time` / `time-core` / `time-macros`
# crates demand rustc 1.88.0+, and `icu_provider` / `idna_adapter`
# demand 1.86+. 1.83 fails the manifest parse (`edition2024` feature),
# 1.85 fails the rustc-version check, 1.90 is the lowest common
# stable that satisfies every transitive lower bound today.
FROM rust:1.90-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release --bin jwc-registry

# --- runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates wget && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/jwc-registry /usr/local/bin/jwc-registry
COPY static /app/static
ENV REGISTRY_BIND=0.0.0.0:8080 \
    REGISTRY_STORAGE_PATH=/var/lib/jwc-registry/storage
RUN mkdir -p /var/lib/jwc-registry/storage
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s \
    CMD wget -q -O- http://127.0.0.1:8080/healthz || exit 1
CMD ["jwc-registry"]
