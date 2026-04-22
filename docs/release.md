# Releasing `api-server`

This runbook covers cutting a release of the `api-server` crate and
pulling the published image on the deploy host (BigMac). The publish
pipeline lives at `.github/workflows/publish-api.yml` and pushes a
multi-arch image to GitHub Container Registry (GHCR):

    ghcr.io/adamgell/cmtraceopen-api

Supported platforms: `linux/amd64`, `linux/arm64`.

## Cutting an api-server release

1. Bump the version in `crates/api-server/Cargo.toml`:

    ```toml
    [package]
    name = "api-server"
    version = "0.1.0"   # ← bump this
    ```

    Commit to `main` (or merge via PR):

    ```sh
    git add crates/api-server/Cargo.toml Cargo.lock
    git commit -m "api-server: bump to 0.1.0"
    git push origin main
    ```

2. Update `CHANGELOG.md`.

    We don't currently maintain a CHANGELOG for api-server. Follow-up:
    add one under `crates/api-server/CHANGELOG.md` and reference it
    from the release notes. For now, write the highlights into the
    annotated tag message.

3. Tag the release. The pipeline triggers on tags matching `api-v*`.
   The `api-v` prefix disambiguates api-server releases from any
   future viewer / wasm-parser release tags.

    ```sh
    git tag api-v0.1.0 -m "api-server v0.1.0"
    git push origin api-v0.1.0
    ```

    Pre-release tags are supported (`api-v0.2.0-rc.1`). The `latest`
    tag is only attached to stable semver releases.

4. Watch the publish run:

    <https://github.com/adamgell/cmtraceopen-web/actions>

    The `publish-api-server` job builds both architectures under QEMU
    and pushes to GHCR. Expect ~15–25 min for the ARM64 leg (emulated
    Rust build).

5. Verify the image is listed under the repo's Packages:

    <https://github.com/adamgell?tab=packages&repo_name=cmtraceopen-web>

## Re-publishing without a new tag

If a publish run fails mid-push or you need to rebuild a tag's image,
use the `workflow_dispatch` trigger from the Actions UI. It accepts a
`push` input — set it to `false` for a build-only dry run (validates
multi-arch + Dockerfile without touching GHCR).

## Pulling the image on BigMac

    docker pull ghcr.io/adamgell/cmtraceopen-api:latest

Or a specific version:

    docker pull ghcr.io/adamgell/cmtraceopen-api:0.1.0

Docker's manifest selects the right arch automatically (amd64 on Linux
x86\_64, arm64 on Apple Silicon / Raspberry Pi 4).

If the package is set to Private in GHCR, the deploy host will need to
authenticate before pulling:

    echo "$GHCR_PAT" | docker login ghcr.io -u <your-username> --password-stdin

where `GHCR_PAT` is a GitHub PAT with `read:packages` scope. Making
the package Public removes this step entirely — recommended for OSS.

## Switching compose to a pull-based deploy

Today's `docker-compose.yml` builds `api-server` from source on every
`docker compose up -d`. Once a published image exists, the compose
file (or an Ansible `compose_stack`-managed override) can swap to
pulling:

```yaml
services:
  api-server:
    image: ghcr.io/adamgell/cmtraceopen-api:latest
    # (remove the `build:` block, or keep both with `build:` as a
    # fallback — `docker compose pull` will still prefer the registry
    # image when `image:` is set.)
```

Follow-up: wire this into the Ansible `compose_stack` role so
`ansible-playbook deploy.yml -e api_version=0.1.0` pins the running
tag per host.
