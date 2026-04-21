# colima_dev_env

Installs [Colima](https://github.com/abiosoft/colima) + Docker CLI + Compose + buildx on a macOS host and wires `~/.docker/config.json` so the Docker CLI talks to Colima's socket.

## What it handles

- Brew formulas: `colima`, `docker`, `docker-compose`, `docker-buildx`, `jq`.
- Patches `~/.docker/config.json`:
  - Drops a stale `"credsStore": "desktop"` (common after a Docker Desktop uninstall — blocks `docker pull` on public images with `docker-credential-desktop: not found`).
  - Sets `"cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]` so brew-installed Compose + buildx plugins are discovered.
- Starts Colima with the configured CPU / memory / disk. No-op if already running.

## Why buildx is required

Dockerfiles that use `--mount=type=cache,...` for Cargo registry / build-artifact caching need BuildKit. Without `docker-buildx` installed **and** discoverable via `cliPluginsExtraDirs`, those builds fall back to the legacy builder and fail with `the --mount option requires BuildKit`.

## Usage

```yaml
- hosts: mac_hosts
  roles:
    - role: colima_dev_env
      vars:
        colima_dev_env_cpu: 6
        colima_dev_env_memory: 8
```

All vars in `defaults/main.yml`.

## Tags

- `colima_dev_env` — everything in the role
- `docker_config` — just the `~/.docker/config.json` patch
- `colima` — just the Colima install / start

## Notes

- Idempotent. Safe to re-run.
- Apple Silicon defaults. On Intel Macs, override `colima_dev_env_brew_bin` / `colima_dev_env_brew_prefix` to `/usr/local/...`.
- Colima autostart-on-login (LaunchAgent at `~/Library/LaunchAgents/`) is not managed by this role. Run `colima start` post-login, or wire a LaunchAgent via the `launchd_service` role if you need it to survive reboot + login.
