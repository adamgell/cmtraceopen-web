# scripts/

One-shot operational helpers for the already-onboarded BigMac26 runner.

## `redeploy.sh` — refresh the cmtraceopen-web stack on BigMac26

A zero-ceremony wrapper around the "SSH into BigMac, git pull, `docker compose
up -d --build`, smoke-test the endpoints" dance. Replaces the ad-hoc agents we
kept firing for redeploys.

### Usage

```bash
# default: deploy main with a full rebuild
./redeploy.sh

# deploy a feature branch for testing
./redeploy.sh --branch feat/something

# fast restart after a host-side config change (no --build)
./redeploy.sh --skip-build

# skip the post-deploy curl checks
./redeploy.sh --no-smoke

# help
./redeploy.sh --help
```

The script resolves its own location, so it works from any cwd:

```bash
dev/bigmac-runner-kit/scripts/redeploy.sh --branch main
```

### What it does

1. SSHes to `Adam.Gell@192.168.2.50` using `~/.ssh/id_ed25519` on the control
   machine by default. Override with `-i PATH` or the `CMTRACE_SSH_KEY` env
   var (e.g. to use the kit-bundled `dev/bigmac-runner-kit/id_ed25519`).
2. On the remote, in `~/repo/cmtraceopen-web`: `git fetch`, checkout the
   target branch, `git pull --ff-only`, refresh submodules, and capture the
   old -> new short SHAs.
3. Rebuilds the compose stack against the colima socket
   (`DOCKER_HOST=unix:///Users/Adam.Gell/.colima/default/docker.sock`):
   `docker compose down` (volumes preserved) then `docker compose up -d
   --build`.
4. Smoke-tests `http://localhost:8080/healthz` and `/readyz` with `jq`,
   plus a grep for the status page `Uptime` line.
5. Prints a summary with the SHA transition and `docker compose ps`.

Every remote command is prefixed with `eval "$(/opt/homebrew/bin/brew
shellenv)"` so brew-provided tools (`git`, `jq`, `docker`, `curl`) are on
PATH under the non-interactive SSH session.

### Prerequisites

- SSH private key at `~/.ssh/id_ed25519` on the control machine (matching
  pubkey already in `~/.ssh/authorized_keys` on BigMac26). Use
  `-i PATH` / `CMTRACE_SSH_KEY` to point at the kit-bundled key or any
  other location.
- BigMac26 reachable at `192.168.2.50` from the control machine.
- colima already running on BigMac26 (the deploy path assumes the Docker
  socket at `~/.colima/default/docker.sock` exists — this script does not
  install or start colima).
- The repo already cloned at `~/repo/cmtraceopen-web` on BigMac26.

### Exit codes

- `0` — deploy succeeded (smoke tests are best-effort and do not gate this).
- `1` — argument parsing error or remote failure (e.g. `docker compose up`
  failed; the last 30 lines of `api-server` logs are dumped).
- `2` — SSH pre-flight failed (key missing, host unreachable, auth refused).

### When to use the Ansible kit instead

This script is a **redeploy shortcut** for an already-onboarded BigMac26. For
first-time onboarding, multi-host workflows, or anything you want to declare
idempotently, use the Ansible path documented in the kit's top-level
[`README.md`](../README.md) (`ansible-playbook playbooks/deploy.yml`). That
path handles the full launchd-service install, not just re-rolling an
existing stack.

### Post-deploy URLs

- Status page:    <http://192.168.2.50:8080/>
- Health probe:   <http://192.168.2.50:8080/healthz>
- Readiness:      <http://192.168.2.50:8080/readyz>
- Adminer:        <http://192.168.2.50:8082/>
