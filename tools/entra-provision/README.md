# entra-provision

Interactive PowerShell provisioner for the two Entra app registrations the
cmtraceopen viewer needs for operator sign-in. Automates the portal flow
described in `docs/provisioning/02-entra-app-registration.md`.

## Prerequisites

- PowerShell 7+ (`pwsh`). Works on macOS, Linux, and Windows.
- An Entra tenant where you hold **Global Administrator** or **Application
  Administrator**.
- Network access to `graph.microsoft.com` and `login.microsoftonline.com`.

The script installs the `Microsoft.Graph.*` modules for the current user
on first run if they aren't already present.

## Usage

```bash
pwsh ./tools/entra-provision/Provision-CmtraceEntra.ps1 -AssignCurrentUserAsAdmin
```

A browser window opens for interactive Entra sign-in. After consent, the
script creates / updates both app registrations, grants tenant-wide
admin consent for the viewer's delegated scope, optionally assigns you
to `CmtraceOpen.Admin`, and writes `<repo-root>/.env.local` with the
three `VITE_ENTRA_*` values.

### Common flags

| Flag | Purpose |
| --- | --- |
| `-TenantId <guid>` | Target a specific tenant instead of the default. |
| `-RedirectUri 'http://localhost:5173/','https://viewer.example.com/'` | Register additional SPA redirect URIs. |
| `-AssignCurrentUserAsAdmin` | Grant yourself `CmtraceOpen.Admin` on the api app. |
| `-SkipAdminConsent` | Leave the delegated scope in per-user-consent mode. |
| `-EnvLocalPath ''` | Don't write `.env.local` (print only). |

## Idempotency

Re-running the script is safe. Apps are matched by display name; existing
registrations are patched in place rather than duplicated. The generated
scope UUID, role UUIDs, and redirect URIs are preserved across runs.

## What it does not do

- Does **not** configure the api-server. The api-server env values
  (`CMTRACE_ENTRA_*`) are printed at the end; paste them into the
  api-server's environment (e.g. the `api-server` service block in
  `docker-compose.yml`) when you want to flip from
  `CMTRACE_AUTH_MODE=disabled` to real JWT validation.
- Does **not** set `Assignment required?` on the viewer enterprise
  application. If you want to gate sign-in to a curated operator list,
  flip that toggle in the portal (see step 5 of the provisioning doc).
- Does **not** provision Cloud PKI / Intune for the agent. That flow is
  covered by `docs/provisioning/03-intune-cloud-pki.md`.
