# CMTrace Open viewer — containerized build.
#
# Three-stage: wasm-pack → vite build → nginx runtime.
#
# Build-time args:
#   VITE_CMTRACE_API_BASE  (default "" = same-origin; nginx proxies /v1 + /healthz)
#   VITE_ENTRA_TENANT_ID
#   VITE_ENTRA_CLIENT_ID
#   VITE_ENTRA_API_SCOPE
#
# Vite inlines these at build time, so rebuild the image when they change:
#   docker compose --env-file .env.local build viewer

# ---------- Stage 1: build cmtrace-wasm ----------
FROM rust:1.90-bookworm AS wasm-build
WORKDIR /src

RUN cargo install wasm-pack --version 0.13.1 --locked

# Copy only what wasm-pack needs: the wasm crate + the cmtraceopen submodule
# (path dep: ../cmtraceopen/crates/cmtraceopen-parser).
COPY cmtrace-wasm /src/cmtrace-wasm
COPY cmtraceopen /src/cmtraceopen

RUN wasm-pack build cmtrace-wasm --target web --release --out-dir /src/pkg

# ---------- Stage 2: build the SPA ----------
FROM node:22-bookworm-slim AS viewer-build
WORKDIR /app

RUN corepack enable

COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

# Source tree needed for vite build + tsc.
COPY tsconfig.json vite.config.ts index.html ./
COPY src ./src
COPY --from=wasm-build /src/pkg ./pkg

ARG VITE_CMTRACE_API_BASE=""
ARG VITE_ENTRA_TENANT_ID=""
ARG VITE_ENTRA_CLIENT_ID=""
ARG VITE_ENTRA_API_SCOPE=""
ENV VITE_CMTRACE_API_BASE=$VITE_CMTRACE_API_BASE \
    VITE_ENTRA_TENANT_ID=$VITE_ENTRA_TENANT_ID \
    VITE_ENTRA_CLIENT_ID=$VITE_ENTRA_CLIENT_ID \
    VITE_ENTRA_API_SCOPE=$VITE_ENTRA_API_SCOPE

RUN pnpm exec tsc --noEmit && pnpm exec vite build

# ---------- Stage 3: runtime ----------
FROM nginx:1.27-alpine AS runtime

COPY infra/viewer/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=viewer-build /app/dist /usr/share/nginx/html

EXPOSE 80

HEALTHCHECK --interval=15s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1/viewer-healthz || exit 1
