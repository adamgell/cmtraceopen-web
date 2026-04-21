# 02 — Entra (Azure AD) App Registration

Provisioning runbook for the Entra application(s) used by the cmtraceopen
platform for operator authentication. Referenced by Wave 2 auth work
(web-viewer bearer-token flow + api-server JWKS validation on query routes).

> This runbook is a project-specific summary. For authoritative details,
> consult Microsoft's official documentation:
>
> - MSAL.js: https://learn.microsoft.com/entra/identity-platform/msal-js-overview
> - JWT validation / JWKS: https://learn.microsoft.com/entra/identity-platform/access-tokens
> - `az ad app`: https://learn.microsoft.com/cli/azure/ad/app

---

## Purpose

cmtraceopen uses a **dual-auth model**:

1. **Agent-to-api-server** — mTLS with a Cloud PKI certificate. Covered in
   `03-cloud-pki-agent-cert.md` (separate doc). Not touched here.
2. **Operator-to-web-viewer** — human operators browsing the web viewer
   authenticate via **Entra OAuth 2.0 Authorization Code + PKCE**. The SPA
   obtains a bearer token from Entra and presents it on every api-server
   query request. The api-server validates the JWT signature via the
   tenant's JWKS endpoint and enforces the `aud` + `iss` + `scp` claims on
   query routes.

**Topology: single-app vs two-app.** This runbook recommends the
**two-app-registration** approach:

- `cmtraceopen-api` — the protected-resource identity. Exposes the
  `CmtraceOpen.Query` scope. Has no redirect URIs, no client secret.
- `cmtraceopen-viewer` — the SPA client identity. Declares the viewer's
  redirect URIs and consumes the `CmtraceOpen.Query` scope above.

Rationale: two apps cleanly separate "who validates tokens" from "who
requests tokens". The API app's Application ID URI (`api://<api-client-id>`)
becomes the `aud` claim the api-server enforces, and the SPA can be
reconfigured (new redirect URIs, new platforms) without touching the
protected-resource contract. A single-app arrangement works for an MVP but
conflates the two concerns and makes it harder to later add a second client
(CLI, mobile viewer, etc.).

---

## Prerequisites

- An Entra tenant where you hold **Global Administrator** or
  **Application Administrator** role (both app creation and admin consent
  require one of these).
- Azure CLI >= 2.55 installed locally (optional; step 4 uses it). Log in
  with `az login --tenant <tenant-id>`.
- A password manager (1Password, Bitwarden, etc.) — tenant/client IDs go
  there, **not** into this repository.

---

## Step 1 — Register the API application (`cmtraceopen-api`)

This is the **backend identity**. The api-server does not run as this app;
the app registration simply names the protected resource and defines its
scopes.

**Portal path:** `Entra ID` → `App registrations` → `New registration`.

| Field | Value |
| --- | --- |
| Name | `cmtraceopen-api` |
| Supported account types | **Single tenant** (Accounts in this organizational directory only) |
| Redirect URI | *(leave blank — this app is never redirected to)* |

After creation, from the app's **Overview** blade, capture:

- **Application (client) ID** — becomes `<api-client-id>` below.
- **Directory (tenant) ID** — becomes `<tenant-id>` below.

### 1a — Expose an API scope

Navigate to **Expose an API** → **Add a scope**.

On first use the portal will prompt to set the **Application ID URI**.
Accept the default `api://<api-client-id>`. Record this — the api-server
validates it as the `aud` claim.

Add the scope:

| Field | Value |
| --- | --- |
| Scope name | `CmtraceOpen.Query` |
| Who can consent? | **Admins and users** |
| Admin consent display name | `Query cmtraceopen logs` |
| Admin consent description | `Allows the signed-in operator to run queries and retrieve CMTrace log data via the cmtraceopen api-server.` |
| User consent display name | `Query logs via cmtraceopen` |
| User consent description | `Let cmtraceopen run log queries on your behalf.` |
| State | **Enabled** |

No other permissions (Microsoft Graph, etc.) are required on this app.

---

## Step 2 — Register the web viewer SPA (`cmtraceopen-viewer`)

This is the **operator-facing client**. MSAL.js in the browser drives the
authorization-code + PKCE flow against this registration.

**Portal path:** `Entra ID` → `App registrations` → `New registration`.

| Field | Value |
| --- | --- |
| Name | `cmtraceopen-viewer` |
| Supported account types | **Single tenant** |
| Redirect URI platform | **Single-page application (SPA)** |
| Redirect URI | `http://localhost:5173/` (local Vite dev) |

After creation, go to **Authentication** and add the additional SPA
redirect URIs:

- `http://localhost:5173/` — local Vite dev server
- `http://192.168.2.50:8080/` — BigMac dev deploy (placeholder; replace
  once a stable host/domain is assigned)
- `https://<prod-domain>/` — production (placeholder; add when the prod
  domain is provisioned)

Under **Authentication → Implicit grant and hybrid flows**: for a pure
PKCE SPA using MSAL.js v2+, leaving both "Access tokens" and "ID tokens"
**unchecked** is correct and recommended. MSAL.js defaults to the
authorization-code + PKCE flow and does not need implicit-grant token
issuance. Only tick these boxes if you deliberately fall back to the
implicit flow (not recommended).

### 2a — Request the API scope

Navigate to **API permissions** → **Add a permission** → **My APIs** →
select `cmtraceopen-api` → **Delegated permissions** → tick
`CmtraceOpen.Query` → **Add permissions**.

Then click **Grant admin consent for \<tenant>** so operators don't see a
per-user consent prompt on first login.

From the **Overview** blade, capture:

- **Application (client) ID** — becomes `<viewer-client-id>` below.

---

## Step 3 — Persist the configuration

### Web viewer (`.env.local`, gitignored on the dev machine)

```env
VITE_ENTRA_TENANT_ID=<tenant-id>
VITE_ENTRA_CLIENT_ID=<viewer-client-id>
VITE_ENTRA_API_SCOPE=api://<api-client-id>/CmtraceOpen.Query
```

Confirm `.env.local` is covered by `.gitignore` before saving. These
identifiers are not secrets (an Entra tenant ID and public-client ID are
safe to leak) but there is still no reason to commit them — each operator
or deployment target sets their own.

### api-server (consumed when Wave 2 auth code lands)

```env
CMTRACE_ENTRA_TENANT_ID=<tenant-id>
CMTRACE_ENTRA_AUDIENCE=api://<api-client-id>
CMTRACE_ENTRA_JWKS_URI=https://login.microsoftonline.com/<tenant-id>/discovery/v2.0/keys
```

The api-server will:

- fetch + cache the JWKS from `CMTRACE_ENTRA_JWKS_URI`,
- verify incoming JWT signatures against those keys,
- reject tokens whose `aud` does not equal `CMTRACE_ENTRA_AUDIENCE`,
- reject tokens whose `iss` does not equal
  `https://login.microsoftonline.com/<CMTRACE_ENTRA_TENANT_ID>/v2.0`,
- require the `scp` claim to contain `CmtraceOpen.Query` on all query
  routes.

No client secret is required anywhere: the SPA uses PKCE, and the
api-server only **validates** tokens against the public JWKS.

---

## Step 4 — CLI alternative (`az ad app`)

Equivalent provisioning via Azure CLI, useful for replicating into a new
tenant or scripting. Values in `<angle brackets>` are placeholders.

```bash
# --- API app ---------------------------------------------------------
az ad app create \
  --display-name cmtraceopen-api \
  --sign-in-audience AzureADMyOrg

# capture its appId
API_APP_ID=$(az ad app list --display-name cmtraceopen-api \
             --query '[0].appId' -o tsv)

# set the identifier URI to api://<api-client-id>
az ad app update --id "$API_APP_ID" \
  --identifier-uris "api://$API_APP_ID"

# expose the CmtraceOpen.Query scope (edit the generated JSON, then):
az ad app update --id "$API_APP_ID" --set api=@api-scope.json
# api-scope.json defines oauth2PermissionScopes with:
#   value = "CmtraceOpen.Query", type = "User", isEnabled = true,
#   adminConsentDisplayName/Description + userConsentDisplayName/Description.

# --- SPA (viewer) app -----------------------------------------------
az ad app create \
  --display-name cmtraceopen-viewer \
  --sign-in-audience AzureADMyOrg \
  --spa-redirect-uris \
      http://localhost:5173/ \
      http://192.168.2.50:8080/

VIEWER_APP_ID=$(az ad app list --display-name cmtraceopen-viewer \
                --query '[0].appId' -o tsv)

# grant delegated permission on CmtraceOpen.Query
# (scope-id is the guid minted by the API app when the scope was created;
#  read it back with: az ad app show --id "$API_APP_ID" \
#                       --query 'api.oauth2PermissionScopes[0].id' -o tsv)
SCOPE_ID=$(az ad app show --id "$API_APP_ID" \
           --query 'api.oauth2PermissionScopes[0].id' -o tsv)

az ad app permission add --id "$VIEWER_APP_ID" \
  --api "$API_APP_ID" \
  --api-permissions "$SCOPE_ID=Scope"

az ad app permission admin-consent --id "$VIEWER_APP_ID"
```

`az ad sp create-for-rbac` is **not** needed for either of these apps —
neither one uses a client secret or Azure-RBAC role assignment. Only
create a service principal if a future component needs to call the
Microsoft Graph or other RBAC-protected resource.

---

## Step 5 — Assign operator users (nice-to-have)

By default, any user in the tenant can sign in to the SPA. To restrict
sign-in to a curated operator list:

1. In `cmtraceopen-viewer` → **Overview** → **Managed application in
   local directory**, click through to the **Enterprise Application**
   view.
2. **Properties** → set **Assignment required?** = **Yes**.
3. **Users and groups** → **Add user/group** → select the operators
   (initial list: `Adam.Gell`; grow as the team scales) → assign the
   default **User** role → **Assign**.

### Optional future extension — app roles / RBAC

The MVP only distinguishes authenticated operators from unauthenticated
traffic. A future iteration can define **app roles** on
`cmtraceopen-api` for finer-grained authorization, e.g.:

- `cmtraceopen.admin` — full access including ingest-side admin routes.
- `cmtraceopen.viewer` — read-only query access.

When added, the api-server would enforce the `roles` claim alongside
`scp`. Do **not** implement this in the MVP; documented here so the Wave
2 agent knows the extension point.

---

## Verification

1. **Tenant metadata reachable.** From any browser:

   ```
   https://login.microsoftonline.com/<tenant-id>/v2.0/.well-known/openid-configuration
   ```

   Returns a JSON document including `jwks_uri`, `issuer`,
   `authorization_endpoint`, `token_endpoint`. Confirms the tenant ID is
   correct and the JWKS URI matches `CMTRACE_ENTRA_JWKS_URI`.

2. **MSAL.js smoke test.** In a browser console on the dev viewer:

   ```js
   const app = new msal.PublicClientApplication({
     auth: {
       clientId: "<viewer-client-id>",
       authority: "https://login.microsoftonline.com/<tenant-id>",
       redirectUri: "http://localhost:5173/",
     },
   });
   await app.initialize();
   ```

   Instantiation + `initialize()` resolves without error. A subsequent
   `loginPopup({ scopes: ["api://<api-client-id>/CmtraceOpen.Query"] })`
   returns an access token whose `aud` equals `api://<api-client-id>`
   and whose `scp` includes `CmtraceOpen.Query` (decode at jwt.ms to
   confirm).

3. **Admin consent granted.** In `cmtraceopen-viewer` → **API
   permissions**, the `CmtraceOpen.Query` row shows a green tick under
   **Status** (`Granted for <tenant>`).

---

## "Done" criteria

- [ ] `cmtraceopen-api` app registration exists; `CmtraceOpen.Query`
      scope is exposed and enabled.
- [ ] `cmtraceopen-viewer` app registration exists with SPA redirect
      URIs configured; admin consent granted for `CmtraceOpen.Query`.
- [ ] At least one operator user is assigned to the viewer's Enterprise
      Application (if `Assignment required` was turned on).
- [ ] Tenant ID + both client IDs + Application ID URI captured into the
      team password manager. **Not** committed to this repository.
- [ ] Tenant OpenID-configuration endpoint returns valid JSON.

---

## Constraints for this document

- **No real tenant IDs or client IDs in this file** — placeholders only.
  Concrete values live in the password manager and in each machine's
  `.env.local` / api-server environment.
- **No secrets.** The SPA is a public client using PKCE, and the API app
  validates JWTs against the public JWKS endpoint; neither requires a
  client secret. If a future component ever needs one, document its
  rotation policy separately.
- Microsoft's MSAL.js and access-token-validation documentation are the
  authoritative references. This runbook is a project-specific summary
  intended to make Wave 2 auth work reproducible.
