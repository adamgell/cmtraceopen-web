# bigmac-runner-kit

Self-contained starter for SSH-ing into **BigMac26** (Adam's always-on Mac runner at `192.168.2.50`) and deploying always-on services via launchd.

Extracted from `MediaStackAutopilot/roles/mac_runner` + `roles/beszel_agent/tasks/darwin.yml`. Generic — not tied to MLX or Beszel specifically.

## Target host

| Field | Value |
|---|---|
| IP | `192.168.2.50` |
| DNS | `runner.gell.one`, `bigmac.gell.one` (both → `.50`, NOT through NPM) |
| User | `Adam.Gell` |
| Auth | SSH key (`~/.ssh/id_ed25519` on the control Mac is already authorized) |
| OS | macOS (arm64, Apple Silicon) |
| Shell | zsh |
| Homebrew prefix | `/opt/homebrew` |

Quick connection test from the control Mac:

```bash
ssh Adam.Gell@192.168.2.50 'uname -a'
```

## What "launch a service" means on this host

**Docker is NOT installed on BigMac26.** For always-on background services on macOS the native pattern is a **user-scope LaunchAgent** (`~/Library/LaunchAgents/<label>.plist`) loaded with `launchctl load -w`. That's what this kit gives you.

If you genuinely need Docker, install colima or Docker Desktop first (out of scope here — add a prereq step to your playbook).

## Two deploy paths

1. **Ansible** (recommended) — idempotent, declarative. See `playbooks/deploy.yml` + `roles/launchd_service`.
2. **Bash one-liner** — `examples/shell-only-deploy.sh` shows the raw `scp` + `launchctl` dance if you don't want an Ansible dependency.

## Quick redeploy

For the already-onboarded BigMac26, `scripts/redeploy.sh` is a one-liner wrapper around the `git pull` + `docker compose up -d --build` + smoke-test dance for the cmtraceopen-web stack. Use the Ansible path above for onboarding or multi-host work; use this script when the host is live and you just want the latest `main` (or a feature branch) rolled out. See [`scripts/README.md`](scripts/README.md) for flags and exit codes.

## Prereqs on the control machine

- `ansible` (`brew install ansible`) — only for the Ansible path.
- SSH key at `~/.ssh/id_ed25519` authorized on BigMac26. Already set up on Adam's control Mac; a new machine needs `ssh-copy-id Adam.Gell@192.168.2.50` once.

## Prereqs already handled on BigMac26 (don't redo)

- `pmset -c sleep 0 disablesleep 1` — never sleeps on AC.
- DHCP reservation for the Wi-Fi MAC on the Windows DHCP server at `192.168.2.7`.
- Homebrew installed at `/opt/homebrew`.
- `launchctl setenv OLLAMA_HOST "0.0.0.0:11434"` (Ollama already bound to LAN).

## Gotchas (carried over from `mac_runner`)

- **LaunchAgent needs a logged-in user session.** If BigMac26 reboots and nobody logs in, nothing starts. FileVault is on (non-negotiable) so auto-login isn't an option. If your service MUST survive unattended reboots, move it to a LaunchDaemon in `/Library/LaunchDaemons/` (runs as root, no login needed; watch `$HOME` and file ownership).
- **GPU memory is shared.** The MLX Gemma-4-31B preload eats most of the unified memory. Loading a big Ollama model on top will OOM. Plan accordingly.
- **`git.gell.one` SSH doesn't work through NPM.** If your service pulls from Forgejo, `git push` from BigMac26 needs this in `~/.ssh/config`:
  ```
  Host git.gell.one
      HostName 192.168.2.230
      User git
  ```
- **Removing a service is NOT automatic.** Deleting the role invocation won't clean up `~/Library/LaunchAgents/<label>.plist`. Unload + `rm` manually.

## Usage

```bash
cd bigmac-runner-kit
ansible-playbook playbooks/deploy.yml                      # deploys the example
ansible-playbook playbooks/deploy.yml --check              # dry-run
ansible-playbook playbooks/deploy.yml -t launchd_service   # same, scoped
```

To add your own service, copy `examples/hello-service.yml` and edit the `launchd_services` list. See the role's `defaults/main.yml` for the full parameter schema.
