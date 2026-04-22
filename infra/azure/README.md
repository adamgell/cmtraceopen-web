# cmtraceopen-api — Azure Container Apps + Application Gateway Terraform module

This module stands up an internet-reachable production deploy of `cmtraceopen-api` on:

- **Application Gateway v2 (WAF_v2)** — terminates TLS, enforces mTLS on `/v1/ingest/*` against the Cloud PKI client-cert chain, forwards the verified peer cert to the backend via the `X-ARR-ClientCert` header.
- **Container Apps** — runs the api-server container (single revision, autoscale 1-5 replicas by default).
- **Postgres Flexible Server** — VNet-injected, AAD-admin enabled, private DNS.
- **Storage account** — `bundles` container with private endpoint + lifecycle policy (Cool after 30d, delete after 90d).
- **Key Vault** — frontend cert, trusted client CA, Postgres connection string. Private endpoint.
- **Log Analytics workspace** — diagnostic sink for AppGW, ACA, Postgres, Storage.

The full architecture is documented in [`docs/wave4/05-azure-deploy.md`](../../docs/wave4/05-azure-deploy.md).

---

## Quickstart

The examples below assume you're integrating into an existing Terraform repo (the user's case). For a standalone trial, see `examples/prod/`.

### Option A: Vendor as a git module (recommended)

```hcl
module "cmtrace_api" {
  source = "git::https://github.com/adamgell/cmtraceopen-web.git//infra/azure?ref=v0.1.0"

  environment                  = "prod"
  resource_group_name          = "rg-cmtraceopen-prod-cus"
  entra_tenant_id              = var.entra_tenant_id
  entra_audience               = "api://cmtrace-api"
  kv_admin_object_id           = var.kv_admin_group_object_id
  postgres_aad_admin_object_id = var.pg_admin_group_object_id
  frontend_fqdn                = "api.cmtrace.example.com"
}
```

Pinning to a tag (`?ref=v0.1.0`) means upgrades are deliberate. Drop the `ref=` to chase `main`.

### Option B: Copy + commit into your monorepo

`cp -r infra/azure /path/to/your/repo/modules/cmtrace_api/`. Source becomes `./modules/cmtrace_api`. Choose this if you need to fork module behaviour.

---

## Required inputs

| Variable | What |
|---|---|
| `environment` | Short env name (`pilot`, `prod`) — feeds resource naming |
| `resource_group_name` | RG you've already created (module does not own RG) |
| `entra_tenant_id` | Entra tenant GUID for operator bearer-token validation |
| `entra_audience` | App ID URI of the cmtrace API app reg (e.g. `api://cmtrace-api`) |
| `kv_admin_object_id` | Entra group that gets KV admin (cert/secret upload) |
| `postgres_aad_admin_object_id` | Entra group that becomes Postgres AAD admin |
| `frontend_fqdn` | Customer-facing hostname (you manage DNS) |

See `variables.tf` for the full list with defaults.

---

## Pre-apply: cert + CA upload

Two secrets must exist in Key Vault before `apply` will succeed (the AppGW SSL config reads them via `data` blocks):

| KV secret name (default) | What | How |
|---|---|---|
| `appgw-frontend-cert` | PFX of the public TLS cert AppGW serves on 443 | `az keyvault secret set --vault-name <kv> --name appgw-frontend-cert --file cert.pfx --encoding base64` |
| `appgw-client-root-ca` | Cloud PKI Root + Issuing chain (PEM, concatenated) | `cat root.pem issuing.pem | az keyvault secret set --vault-name <kv> --name appgw-client-root-ca --file /dev/stdin` |

KV is created on the **first** `apply`. The flow is:

1. `terraform apply -target=module.cmtrace_api.module.keyvault` — creates KV + private endpoint.
2. Upload both secrets via `az keyvault secret set`.
3. `terraform apply` — full apply now succeeds.

---

## Outputs

| Output | What |
|---|---|
| `appgw_public_ip` | Static IP — point your DNS A record here |
| `ingress_url` | `https://<frontend_fqdn>` — give to operators + agents |
| `container_app_fqdn` | Internal ACA ingress FQDN (do NOT expose externally) |
| `managed_identity_principal_id` | api-server's MI object ID — useful for downstream RBAC |
| `key_vault_uri` | KV URI — for `az keyvault` cert/secret rotations |
| `postgres_server_fqdn` | Private FQDN of the DB |

---

## Tradeoffs baked into the defaults

- **VNet topology**: single VNet with four subnets, not hub/spoke. Promote to hub/spoke by pulling the AppGW + KV PE subnets into a hub VNet variable + adding `azurerm_virtual_network_peering`. None of the resource IDs change.
- **mTLS routing**: dual-listener pattern on a single FQDN. Path `/v1/ingest/*` requires client cert; everything else accepts bearer-only. See `modules/appgw/main.tf` for the rationale (Microsoft-blessed pattern as of mid-2025).
- **Postgres authn**: defaults to local-login + AAD-admin (both enabled). When the api-server gains `azure-identity` token-auth wiring, flip `password_auth_enabled = false` inside the module and the local password becomes break-glass-only.
- **KV RBAC**: `rbac_authorization_enabled = true` (azurerm 4.x rename — was `enable_rbac_authorization` in 3.x).
- **Storage shared keys**: disabled (`shared_access_key_enabled = false`). Only managed-identity auth works against the storage account. Matches the api-server's `CMTRACE_AZURE_USE_MANAGED_IDENTITY=true` posture.
- **WAF mode**: `Prevention` (blocks). Switch to `Detection` for the first 48h of any new ruleset to baseline false positives, then back to `Prevention`.

---

## Cost expectation

| Profile | Components | Approx monthly |
|---|---|---|
| Pilot | Consumption ACA (1-2 replicas), B1ms Postgres, 1-2 AppGW capacity units | $300-400 |
| Production | D4 workload-profile ACA (2-8 replicas), D2ds_v4 Postgres, 2-10 AppGW units | $700-1000 |

Storage + LAW + KV are <$20/mo combined at pilot scale. AppGW is the dominant line item until ACA replica count climbs.

---

## Validation

```bash
cd infra/azure && terraform fmt -check
cd infra/azure/examples/prod && terraform init -backend=false && terraform validate
```

CI doesn't apply — there are no Azure creds at `terraform plan` time in this repo. Validation is contract-only.

---

## Provider versions

- `azurerm ~> 4.0` — the surface used here is stable in 4.0+. We use `azurerm_container_app` (mature in 4.x) and the new KV-secret-reference syntax for ACA secrets.
- `azapi ~> 2.0` — held in reserve for ACA features azurerm doesn't yet expose. Currently the module ships entirely on azurerm; azapi is wired into `versions.tf` so callers can layer azapi resources for preview features without re-pinning.
- `random ~> 3.6` — for the Postgres admin password.
