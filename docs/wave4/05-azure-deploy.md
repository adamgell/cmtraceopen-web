# Wave 4 — Azure deploy: ACA + Application Gateway with mTLS

Status: design + Terraform module shipped (`infra/azure/`); apply gated on **api-server code change** (see §10) before agents can talk through AppGW.

This runbook is the operator-facing companion to the Terraform module under `infra/azure/`. The module is the source of truth for what gets created; this doc covers the why, the order, the things you do outside Terraform, and the cutover steps.

---

## 1. Goal

Stand up an internet-reachable `cmtraceopen-api` so:

- Cloud-PKI-issued device certs handshake against Application Gateway, not the api-server.
- AppGW forwards the verified peer cert to ACA via the `X-ARR-ClientCert` header.
- The api-server keeps its existing SAN-URI extraction (`crates/api-server/src/auth/device_identity.rs`) but reads from the header instead of from the in-process TLS handshake.
- Operator query routes (`/v1/devices`, `/v1/sessions/*`, `/v1/admin/*`) keep using Entra bearer tokens — no client cert needed.
- Bundles land in Azure Blob Storage (`bundles` container) over the Container Apps managed identity. Postgres metadata stays VNet-private.

The architecture is the locked design from the Wave 3 → Wave 4 plan:

```
Agents (Cloud PKI client cert)
  │ HTTPS + mTLS
  ▼
Application Gateway v2 (WAF_v2)         ─── Cloud PKI Root + Issuing CAs uploaded to KV
  │ HTTP (inside VNet) + X-ARR-ClientCert
  ▼
Container Apps (cmtraceopen-api, single revision)
  ├── Managed identity → Postgres (private endpoint, sslmode=require)
  └── Managed identity → Blob Storage (private endpoint)
```

---

## 2. Prereqs

- Azure subscription with Owner role on a resource group (module does not own RG lifecycle).
- The user's existing Terraform environment (state account, providers, naming conventions). This module is designed to be dropped in as a `module "cmtrace_api"` block.
- A custom domain you control DNS for (the customer-facing FQDN). The module emits the AppGW public IP — DNS cutover is a manual step out-of-Terraform.
- Cloud PKI Root + Issuing CA exported as PEM. Per `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`, both download from the Intune admin centre's "Download certificate" button on the CA tile. Save as `gell-pki-root.pem` and `gell-pki-issuing.pem`.
- A public TLS cert for the frontend listener as a PFX. Either:
  - **BYO** — buy/issue a cert against your domain, export as PFX with the private key.
  - **Let's Encrypt** — see §12 (deferred; cert-manager-style automation not in this PR).
- Two Entra groups created in advance:
  - **Key Vault admin group** — its object ID is `kv_admin_object_id`; lets ops upload + rotate certs/secrets.
  - **Postgres AAD admin group** — its object ID is `postgres_aad_admin_object_id`; lets DBA / on-call run psql against the DB.

---

## 3. Module integration

### Recommended: vendor as a git module

In your existing Terraform repo:

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
  cors_origins                 = ["https://cmtrace.example.com"]
}
```

Pinning to `?ref=v0.1.0` makes upgrades opt-in. Drop `ref=` to chase `main`.

### Alternative: copy + commit

`cp -r infra/azure /path/to/your/repo/modules/cmtrace_api/` and set `source = "./modules/cmtrace_api"`. Choose this if you need to fork module behaviour. Lose the easy upstream-merge path.

A complete example caller lives in `infra/azure/examples/prod/`.

---

## 4. Variables (the ones you'll actually touch)

| Input | Default | Notes |
|---|---|---|
| `environment` | (required) | `pilot`, `prod`. Lowercase, ≤12 chars |
| `location` | `centralus` | Override to match your hub region |
| `resource_group_name` | (required) | RG must exist |
| `entra_tenant_id` | (required) | For operator JWT validation |
| `entra_audience` | (required) | App ID URI of the API app reg |
| `cors_origins` | `[]` | Add the viewer's public origin |
| `kv_admin_object_id` | (required) | Entra group, not a person |
| `postgres_aad_admin_object_id` | (required) | Entra group, not a person |
| `frontend_fqdn` | (required) | What customers + agents type |
| `frontend_cert_kv_secret_name` | `appgw-frontend-cert` | KV secret you upload before applying |
| `client_root_ca_kv_secret_name` | `appgw-client-root-ca` | KV secret you upload before applying |
| `image` | `ghcr.io/adamgell/cmtraceopen-api:v0.1.0` | Bump after each GHCR publish |
| `aca_use_workload_profile` | `false` | Flip `true` for prod (predictable latency) |
| `aca_min_replicas` | `1` | Bump to 2 for prod redundancy |
| `aca_max_replicas` | `5` | |
| `aca_cpu` / `aca_memory` | `0.5` / `1Gi` | `2.0` / `4Gi` for prod |
| `postgres_sku_name` | `B_Standard_B1ms` | `GP_Standard_D2ds_v4` for prod |
| `appgw_capacity_min` / `_max` | `1` / `10` | Bump min to 2 for prod |
| `crl_urls` | live Cloud PKI defaults | Override only if pointing at a different PKI |
| `crl_fail_open` | `false` | Keep false in prod |
| `kv_allow_public_access` | `false` | Leave false in prod; flip true only for the cert-upload window if you don't have a jumphost |

Full list in `infra/azure/variables.tf`.

---

## 5. Apply order

Cold-start total: **~30 minutes** (AppGW v2 alone takes 15-20 min).

```bash
cd infra/<your-env>
terraform init
terraform plan -out=cmtrace.plan
```

The first apply has a chicken-and-egg: AppGW reads frontend cert + client CA from KV via `data` blocks, but KV is also created by this module. Use targeted apply on the first run only:

```bash
# 1. Create KV (and its private endpoint).
terraform apply -target=module.cmtrace_api.module.keyvault

# 2. Upload the two secrets (see §6).

# 3. Full apply.
terraform apply cmtrace.plan
```

Subsequent applies are single-step.

The 15-20 min wall time on AppGW provisioning is unavoidable. ACA, Postgres flex, and KV all come up in <5 min. Storage + private endpoints are near-instant.

---

## 6. Cert + CA upload flow

Both secrets need to land in KV before step 3 of the apply. The module does NOT manage these secret values (they're sensitive operator-uploaded material).

### 6a. Frontend TLS cert (PFX)

```bash
# Convert your PEM cert + key to PFX if needed.
openssl pkcs12 -export -out cert.pfx \
  -inkey privkey.pem -in fullchain.pem \
  -password pass:"$(openssl rand -base64 24)"

# Upload as a base64-encoded KV secret.
az keyvault secret set \
  --vault-name <kv-name-from-tf-output> \
  --name appgw-frontend-cert \
  --file cert.pfx \
  --encoding base64
```

AppGW reads via `versionless_id` so future cert rotations are zero-touch on the Terraform side — just `az keyvault secret set` again.

### 6b. Trusted client CA bundle (PEM)

For the live Gell Cloud PKI — concatenate Root + Issuing into one bundle:

```bash
cat gell-pki-root.pem gell-pki-issuing.pem > cloud-pki-bundle.pem

az keyvault secret set \
  --vault-name <kv-name-from-tf-output> \
  --name appgw-client-root-ca \
  --file cloud-pki-bundle.pem
```

AppGW uses this bundle to verify client cert chains. The `verify_client_cert_issuer_dn = true` setting on the SSL profile means the cert's issuer DN must match one of the CAs in the bundle exactly (string-compare on subject DN), so include both Root + Issuing rather than just Root.

---

## 7. DNS cutover

After the full apply completes:

```bash
terraform output appgw_public_ip   # e.g. 20.51.1.42
terraform output appgw_public_fqdn # cmtrace-prod-cus-appgw.centralus.cloudapp.azure.com
```

1. **Smoke-test against the Azure-managed FQDN first** — point a test agent at `https://cmtrace-prod-cus-appgw.centralus.cloudapp.azure.com` (the listener won't match because SNI is `frontend_fqdn`-bound; this confirms TLS handshake reaches AppGW). Use `--resolve` in curl to fake the SNI:
   ```bash
   curl -v --resolve api.cmtrace.example.com:443:20.51.1.42 \
     --cert client.pem --key client.key \
     https://api.cmtrace.example.com/healthz
   ```
2. **Create the A record** at your DNS provider:
   - `api.cmtrace.example.com  A  <appgw_public_ip>` (TTL: 300 during pilot, 3600 after stable).
3. **Update the agent config** to point at the new FQDN. For the BigMac26-pinned dev agents, this is `crates/agent/src/config.rs`'s default endpoint (rebuild + redeploy).
4. **Decommission BigMac26 ingest** only after 24h of clean traffic on the new FQDN.

---

## 8. Smoke test

End-to-end ingest from a Windows test VM (per `docs/provisioning/04-windows-test-vm.md`):

1. Provision the test VM, deploy the cert profile + agent MSI via Intune (`docs/provisioning/05-intune-graph-deploy.md`).
2. Trigger a bundle: open a CMTrace log in the agent context (or run the test harness in `tools/`).
3. From an operator workstation:
   ```bash
   # Get bearer token via az login -t <tenant>
   TOKEN=$(az account get-access-token --resource api://cmtrace-api --query accessToken -o tsv)

   curl -H "Authorization: Bearer $TOKEN" \
     https://api.cmtrace.example.com/v1/devices
   ```
4. Confirm the test device appears with a session within ~30s of bundle finalize.
5. Check `https://api.cmtrace.example.com/metrics` → `cmtrace_ingest_finalize_total` should be >0.

If the bundle ingests but the device doesn't appear: check `cmtrace_ingest_finalize_errors_total` and the ACA logs in LAW (`KQL: ContainerAppConsoleLogs_CL | where ContainerName_s == "api"`).

---

## 9. Rollback

`terraform destroy` removes everything the module created. With `purge_protection_enabled = true` on KV, the vault enters a 7-day soft-delete window — re-creating with the same name in <7 days requires `az keyvault recover` first.

Agents queue bundles locally during outage (per the Wave 3 agent design). Restoration is `terraform apply` of a known-good plan; agents resume on next ingest interval.

For partial rollback (e.g. revert just the api-server image without touching infra), bump `image = "ghcr.io/adamgell/cmtraceopen-api:<previous-tag>"` and `terraform apply` — only the ACA app's revision changes.

---

## 10. Code changes flagged as **required follow-up** in api-server

This Terraform module assumes the api-server can read the verified peer cert from a forwarded HTTP header instead of doing its own TLS termination. The current code (Wave 3) extracts the cert from the in-process rustls handshake (`crates/api-server/src/tls.rs` + `crates/api-server/src/auth/device_identity.rs`). Three changes are needed in a separate PR before this deploy is functional for ingest:

1. **New env var `CMTRACE_PEER_CERT_HEADER`** in `crates/api-server/src/config.rs`:
   - Default `None` (preserves Wave 3 behaviour).
   - When set (the Terraform module sets `X-ARR-ClientCert`), the api-server reads the leaf cert from the named request header instead of from `PeerCertChain`.
   - Validation: when `CMTRACE_PEER_CERT_HEADER` is set AND `CMTRACE_TLS_ENABLED = true`, reject as ambiguous (which one wins?). The deploy uses `CMTRACE_TLS_ENABLED = false`.
2. **Header parsing in `crates/api-server/src/auth/device_identity.rs`**:
   - Accept either source: `PeerCertChain` extension (current) OR forwarded header.
   - The header value is PEM (`-----BEGIN CERTIFICATE-----\n...`); strip headers + newlines, base64-decode, then feed the same DER bytes to the existing SAN URI parser. The existing `parse_san_uri` is unchanged.
   - Set `DeviceIdentitySource::ClientCertificate` for both — operators don't need to know which side terminated TLS.
3. **`CMTRACE_TLS_ENABLED = false` enforcement** when `CMTRACE_PEER_CERT_HEADER` is set:
   - Already what the Terraform module sets.
   - The api-server should warn at startup if `CMTRACE_PEER_CERT_HEADER` is set but the request actually arrives with no header — this catches misconfigured AppGW SSL profiles (mTLS not enforced on the route).

The CRL polling logic in `crates/api-server/src/auth/crl.rs` keeps working unchanged — it operates on the leaf cert DER regardless of how it was sourced.

**Until those three changes ship**, the deploy is functional for operator query routes (Entra bearer auth) but **not** for ingest. The bearer-only paths are enough to run a viewer-side demo end-to-end against this stack.

---

## 11. Cost estimate

| Component | Pilot (consumption ACA, B1ms PG, 1-2 AppGW units) | Production (D4 wp ACA × 2, D2ds_v4 PG, 2-10 AppGW units) |
|---|---|---|
| AppGW v2 base | $200/mo | $200/mo |
| AppGW capacity units | $10-15/mo | $30-100/mo |
| Container Apps | $30/mo (consumption) | $280/mo (D4 × 2 replicas) |
| Postgres flex | $15/mo (B1ms) | $200/mo (D2ds_v4 GP) |
| Storage (LRS, modest blob) | $5/mo | $20/mo |
| Log Analytics (PerGB2018) | $10/mo | $30/mo |
| Key Vault | <$1/mo | <$1/mo |
| Public IP (Standard, static) | $4/mo | $4/mo |
| **Total** | **~$300-400/mo** | **~$700-1000/mo** |

The dominant line is AppGW until ACA replica count climbs. Consider switching to NGINX-on-ACA-Ingress for a 60% cost reduction once mTLS termination is moved into a separate sidecar (~6 month follow-up; not yet designed).

---

## 12. Open questions

| # | Question | Notes |
|---|---|---|
| Q1 | Frontend cert source — Let's Encrypt automation or BYO? | BYO is simpler for a corp domain on a private CA chain. LE via cert-manager-on-AKS or a Function-app cron renewer is a P2 follow-up |
| Q2 | Postgres SKU for prod | Default to `GP_Standard_D2ds_v4`; revisit after pilot data shows `cmtrace_ingest_finalize` p95 |
| Q3 | ACA workload profile vs consumption for prod | Workload profile gives predictable latency for operator queries; consumption tier is fine for ingest spikes (which are the bulk of traffic). Default is workload profile in `examples/prod/`, consumption in pilot |
| Q4 | Autoscale targets | HTTP scale rule defaults to 50 concurrent requests / replica. Tune after pilot |
| Q5 | Hub/spoke vs single VNet | Module ships single VNet. Promote to hub/spoke by extracting AppGW + KV PE subnets to a hub variable + adding peering — additive change, no resource ID churn |
| Q6 | DR region pairing | Out of scope for this PR. Postgres flex supports geo-restore but cross-region failover is operator-driven |
| Q7 | Cert rotation automation | Manual `az keyvault secret set` works; AppGW pulls latest version automatically because we reference `versionless_id` |

---

## Cross-references

- Module: `infra/azure/`
- Module README: `infra/azure/README.md`
- Cloud PKI: `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`
- Wave 4 design: `docs/wave4/01-msi-design.md`, `docs/wave4/02-code-signing.md`, `docs/wave4/03-beta-pilot-runbook.md`, `docs/wave4/04-day2-operations.md`
- API env-var inventory: `crates/api-server/src/config.rs` + `docs/release-notes/api-v0.1.0.md`
- TLS internals (Wave 3): `crates/api-server/src/tls.rs`, `crates/api-server/src/auth/device_identity.rs`

---

## Appendix A — AppGW header-mode env-var quick-reference

This appendix narrows in on the api-server side of the integration: what env
vars the binary reads when running behind AppGW, what the cert header
auto-detection does, and what the operator should put on the AppGW configuration
checklist.

### Env-var matrix

#### Networking

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_LISTEN_ADDR` | `0.0.0.0:8080` | Bind address (plain HTTP, VNet-internal) |

#### TLS / mTLS (in-process — NOT used in AppGW mode)

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_TLS_ENABLED` | `false` | **Must be `false` (or unset) behind AppGW** |
| `CMTRACE_TLS_CERT` | — | Not required in AppGW mode |
| `CMTRACE_TLS_KEY` | — | Not required in AppGW mode |
| `CMTRACE_CLIENT_CA_BUNDLE` | — | Not required in AppGW mode |

> **Note:** If `CMTRACE_PEER_CERT_HEADER` is set, `CMTRACE_TLS_ENABLED` is
> automatically forced to `false` even if set to `true`. A startup warning is
> emitted if both are set so operators know the override happened.

#### AppGW-terminated mTLS (header path)

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_PEER_CERT_HEADER` | _empty_ (disabled) | Name of the header carrying the client cert PEM. Set to `X-ARR-ClientCert` for Azure AppGW. |
| `CMTRACE_TRUSTED_PROXY_CIDR` | _required when header set_ | CIDR of trusted reverse proxies. Only requests whose TCP peer IP falls within this range have the cert header honoured. Typical value: AppGW subnet (e.g. `10.224.0.0/16`). |

#### mTLS enforcement

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_MTLS_REQUIRE_INGEST` | mirrors `CMTRACE_TLS_ENABLED` | If `true`, ingest routes 401 when no device cert can be found via any path. |
| `CMTRACE_SAN_URI_SCHEME` | `device` | Expected URI scheme in the client-cert SAN (e.g. `device://tenant/device-id`). |

### Cert header encoding

AppGW can forward the cert in two ways depending on the gateway SKU and version:

| Encoding | Description | Auto-detected? |
| --- | --- | --- |
| **Raw PEM** | The full `-----BEGIN CERTIFICATE-----` … `-----END CERTIFICATE-----` block as a header value. | Yes |
| **Base64(PEM)** | The PEM string base64-encoded (standard or URL-safe alphabet). | Yes |
| **Base64(DER)** | Raw DER bytes base64-encoded (some proxy configurations). | Yes |

The api-server auto-detects the encoding in this order:

1. If the trimmed value starts with `-----BEGIN` → raw PEM.
2. Base64-decode (standard, then URL-safe) → if result starts with `-----BEGIN` → base64-PEM.
3. Otherwise treat the decoded bytes as raw DER.

### Security considerations

#### Trusted-proxy CIDR enforcement

The cert header is only honoured when the request arrives from an IP within
`CMTRACE_TRUSTED_PROXY_CIDR`. Any request whose TCP peer address is outside
that range has the cert header silently ignored and is treated as if no cert
was presented.

**Important:** Set `CMTRACE_TRUSTED_PROXY_CIDR` to the exact AppGW subnet
rather than `0.0.0.0/0`. A wide CIDR means any host that can reach the
api-server can claim to be any device by forging the header.

#### Mutual exclusivity

Using both `CMTRACE_PEER_CERT_HEADER` and `CMTRACE_TLS_ENABLED=true` is a
configuration error — behind AppGW the api-server MUST run as plain HTTP, not
TLS, because AppGW handles TLS termination. The api-server enforces this:
`CMTRACE_TLS_ENABLED` is overridden to `false` whenever
`CMTRACE_PEER_CERT_HEADER` is set, and a `WARN`-level log entry is emitted at
startup.

### Observability

The `cmtrace_peer_cert_source_total` metric (Prometheus counter) records how
each request's device identity was resolved:

| Label `source=` | Meaning |
| --- | --- |
| `header` | Cert read from the reverse-proxy header (`CMTRACE_PEER_CERT_HEADER`). |
| `tls` | Cert from in-process mTLS session (`CMTRACE_TLS_ENABLED=true`). |
| `none` | No cert found; request proceeded without a device identity (or was rejected). |

Use this metric to confirm that requests are hitting the expected path and to
detect unexpected `none` spikes (e.g. AppGW misconfiguration stopped forwarding
the header).

### AppGW configuration checklist

- [ ] AppGW listener configured with HTTPS and client certificate authentication enabled.
- [ ] Root + Issuing CA certificates uploaded to AppGW as trusted root certificates.
- [ ] HTTP settings have "Override with new host name" disabled (backend speaks plain HTTP on port 8080).
- [ ] Backend HTTP settings include the custom header `X-ARR-ClientCert` forwarding rule (enabled by default on AppGW v2 SKU with mutual authentication).
- [ ] `CMTRACE_TRUSTED_PROXY_CIDR` set to the AppGW subnet address space from the ARM template / VNet configuration.
- [ ] `CMTRACE_PEER_CERT_HEADER` set to `X-ARR-ClientCert`.
- [ ] `CMTRACE_TLS_ENABLED` unset or `false`.
