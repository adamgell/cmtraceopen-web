# syntax=docker/dockerfile:1.7
#
# CMTrace Open — API server image.
#
# Multi-stage build. The builder compiles the `api-server` workspace member;
# the runtime stage is a distroless image so the final container is ~40 MB
# and has no shell, minimizing attack surface.
#
# Build from the cmtraceopen-web repo root (the context needs the workspace
# Cargo.toml and the crates/ tree so the api-server can resolve its path
# dep on common-wire):
#
#   docker buildx build -f crates/api-server/Dockerfile -t cmtraceopen-api:dev .
#
# Multi-platform (Colima on Apple Silicon builds linux/arm64 natively;
# x86_64 via buildx + qemu when needed):
#
#   docker buildx build --platform linux/arm64,linux/amd64 \
#     -f crates/api-server/Dockerfile -t ghcr.io/adamgell/cmtraceopen-api:<tag> \
#     --push .

# ---- Builder ----------------------------------------------------------------
# Pin matches rust-toolchain.toml at the repo root. Without this match,
# rustup auto-installs the channel inside the image on every `cargo`
# invocation — silent slowdown of every build step.
FROM rust:1.90-slim-bookworm AS builder

WORKDIR /workspace

# System deps cargo may invoke. ca-certificates is needed for crates.io HTTPS
# (and for the rustls native-roots loader at runtime); pkg-config is widely
# expected by `-sys` crates.
#
# `cmake` + `nasm` are required by `aws-lc-sys` at build time — reqwest is
# now configured with rustls + aws-lc-rs (matches the agent in PR #37 and
# the Wave 3 mTLS stack), so we no longer need `libssl-dev` (PR #39's
# native-tls hotfix is reverted by the same migration).
#
# `clang` is required by `ring`'s C/asm build script — pulled in transitively
# by `object_store/azure` (the `cloud` feature hard-deps `ring`). Once
# object_store grows a pluggable-crypto knob we should drop this; tracked
# in the Azure-blob feature PR body.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      ca-certificates \
      cmake \
      nasm \
      clang \
 && rm -rf /var/lib/apt/lists/*

# Copy the workspace. .dockerignore keeps the context tight (no node_modules,
# no React frontend, no cmtrace-wasm — the latter has a sibling-repo path
# dep that would break inside the container).
#
# The cmtraceopen submodule IS in context now: api-server's parse-on-ingest
# worker depends on cmtraceopen-parser. Make sure the host has run
# `git submodule update --init --recursive` before kicking off the build,
# otherwise this COPY brings in an empty cmtraceopen/ and the build fails.
COPY . .

# Release build of just the api-server crate (and its transitive dep on
# common-wire). cmtrace-wasm is excluded from this workspace at the root
# Cargo.toml level, so cargo never tries to resolve its cross-repo path dep.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target \
    cargo build --release --locked -p api-server \
 && cp /workspace/target/release/cmtraceopen-api /usr/local/bin/cmtraceopen-api

# ---- Data-dir prep ----------------------------------------------------------
# Distroless has no shell / no mkdir / no chown, so we can't create and
# chown the blob-store root in the runtime stage itself. Stage an empty
# /data owned by UID:GID 65532:65532 (distroless `nonroot`) here, then
# COPY --from= it into the runtime image.
FROM debian:bookworm-slim AS dataprep
RUN mkdir -p /data && chown 65532:65532 /data

# ---- Runtime ----------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12 AS runtime

COPY --from=builder /usr/local/bin/cmtraceopen-api /usr/local/bin/cmtraceopen-api
COPY --from=dataprep --chown=65532:65532 /data /data

USER nonroot:nonroot
EXPOSE 8080
VOLUME ["/data"]

ENV CMTRACE_LISTEN_ADDR=0.0.0.0:8080 \
    CMTRACE_DATA_DIR=/data \
    RUST_LOG=api_server=info,tower_http=info,axum=info,warn

ENTRYPOINT ["/usr/local/bin/cmtraceopen-api"]
