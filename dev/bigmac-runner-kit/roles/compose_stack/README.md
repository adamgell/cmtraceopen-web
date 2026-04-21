# compose_stack

Idempotent git-clone + `docker compose up -d --build` on a macOS host. Assumes [`colima_dev_env`](../colima_dev_env/README.md) has already run (or equivalent tooling is present and `DOCKER_HOST` points at a live Docker daemon).

## Usage

```yaml
- hosts: mac_hosts
  roles:
    - role: colima_dev_env    # prereq
    - role: compose_stack
      vars:
        compose_stack_repo_url: https://github.com/adamgell/cmtraceopen-web.git
        compose_stack_dest: "{{ ansible_env.HOME }}/repo/cmtraceopen-web"
        compose_stack_branch: main
        compose_stack_compose_file: docker-compose.yml
```

## Required vars

| Var | Meaning |
|---|---|
| `compose_stack_repo_url` | Git URL. HTTPS for public repos; SSH for private (auth is your problem). |
| `compose_stack_dest` | Absolute path on target. |
| `compose_stack_branch` | Git branch to check out. |

## Optional vars

| Var | Default | Meaning |
|---|---|---|
| `compose_stack_compose_file` | `docker-compose.yml` | Relative to `dest`. |
| `compose_stack_pull_if_exists` | `true` | If a `.git` dir already exists at `dest`, fast-forward before deploying. Set `false` on hosts where you manage branches by hand. |
| `compose_stack_git_recursive` | `true` | Clone/update with `--recurse-submodules` so gitlink entries (e.g. the `cmtraceopen` parser crate vendored into `cmtraceopen-web`) are populated. Pinned to the recorded submodule pointer — no `track_submodules`. |
| `compose_stack_brew_prefix` | `/opt/homebrew` | Override to `/usr/local` on Intel. |
| `compose_stack_docker_host` | `unix://$HOME/.colima/default/docker.sock` | Must match whatever Docker daemon is running (Colima by default). |

## Behavior

- First run: clones the repo and builds + starts the stack.
- Subsequent runs: fast-forwards if possible, then `docker compose up -d --build` — which rebuilds any changed image layers and recreates affected containers.
- If git auth fails on pull, the role logs a warning and continues with whatever's on disk — useful for CI / unattended environments where transient auth blips shouldn't block a redeploy.

## Tags

- `compose_stack` — everything in the role

## Limitations

- No teardown helper (no `compose_stack_state: absent`). To stop + remove: `docker compose -f <file> down` on the host, or `docker compose -f <file> down -v` to also wipe volumes.
- No log shipping / health-probe task. Add a post_tasks block in the calling playbook (see `playbooks/deploy-api.yml` for an example).
