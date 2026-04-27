###############################################################################
# Inputs to the cmtraceopen-api Azure deploy module.
#
# Caller's existing Terraform repo passes these as `module "cmtrace_api" { ... }`
# inputs. Defaults are biased toward a pilot-cost shape; flip the SKUs in
# `prod`-sized callers.
###############################################################################

# ---------------------------------------------------------------------------
# Naming + tagging
# ---------------------------------------------------------------------------

variable "environment" {
  description = "Short environment name used in resource naming (e.g. \"pilot\", \"prod\"). Lowercase, no dashes."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9]{1,12}$", var.environment))
    error_message = "environment must be 1-12 lowercase alphanumeric chars."
  }
}

variable "location" {
  description = "Azure region for every resource the module creates (e.g. \"centralus\")."
  type        = string
  default     = "centralus"
}

variable "name_prefix" {
  description = "Prefix for resource names. Defaults to \"cmtrace\". Override only if you have a corporate naming convention you must follow."
  type        = string
  default     = "cmtrace"
}

variable "resource_group_name" {
  description = "Existing resource group to deploy into. Module does NOT create the RG — callers manage RG lifecycle in their own state."
  type        = string
}

variable "tags" {
  description = "Tags applied to every resource. Caller typically passes { Environment, System, ManagedBy, CostCenter, Owner }."
  type        = map(string)
  default = {
    System    = "cmtraceopen"
    ManagedBy = "Terraform"
  }
}

# ---------------------------------------------------------------------------
# Container image
# ---------------------------------------------------------------------------

variable "image" {
  description = "Container image reference for the api-server. Default points at the GHCR build published by .github/workflows/publish-api.yml."
  type        = string
  default     = "ghcr.io/adamgell/cmtraceopen-api:v0.1.0"
}

variable "image_pull_secret_name" {
  description = "Optional Key Vault secret holding a registry pull credential PAT (for private GHCR). Empty string disables; module assumes public image when empty."
  type        = string
  default     = ""
}

# ---------------------------------------------------------------------------
# Networking
# ---------------------------------------------------------------------------

variable "vnet_address_space" {
  description = "CIDR for the spoke VNet that hosts ACA + Postgres + Storage PEs."
  type        = list(string)
  default     = ["10.50.0.0/16"]
}

variable "appgw_subnet_cidr" {
  description = "CIDR for the AppGW subnet. AppGW v2 requires /24 minimum to allow autoscale-out room."
  type        = string
  default     = "10.50.0.0/24"
}

variable "aca_subnet_cidr" {
  description = "CIDR for the Container Apps environment subnet. Must be at least /23 for workload-profile envs (Azure requirement)."
  type        = string
  default     = "10.50.2.0/23"
}

variable "postgres_subnet_cidr" {
  description = "CIDR for the Postgres-flexible delegated subnet."
  type        = string
  default     = "10.50.4.0/24"
}

variable "pe_subnet_cidr" {
  description = "CIDR for the private-endpoints subnet (KV + Storage)."
  type        = string
  default     = "10.50.5.0/24"
}

# ---------------------------------------------------------------------------
# Auth (Entra)
# ---------------------------------------------------------------------------

variable "entra_tenant_id" {
  description = "Entra tenant GUID for operator bearer-token validation. Maps to CMTRACE_ENTRA_TENANT_ID."
  type        = string
}

variable "entra_audience" {
  description = "App ID URI of the cmtraceopen API app registration (e.g. \"api://cmtrace-api\"). Maps to CMTRACE_ENTRA_AUDIENCE."
  type        = string
}

# ---------------------------------------------------------------------------
# CORS (viewer)
# ---------------------------------------------------------------------------

variable "cors_origins" {
  description = "Comma-separated list of allowed browser origins for the viewer (e.g. \"https://cmtrace.example.com\"). Maps to CMTRACE_CORS_ORIGINS."
  type        = list(string)
  default     = []
}

# ---------------------------------------------------------------------------
# CRL polling (Cloud PKI)
# ---------------------------------------------------------------------------

variable "crl_urls" {
  description = "CRL distribution points the api-server polls for client-cert revocation. Defaults are the live Gell Cloud PKI Issuing + Root CDN URLs from reference_cloud_pki.md. Override if pointing at a different PKI."
  type        = list(string)
  default = [
    "http://primary-cdn.pki.azure.net/centralus/crls/9a8a2d279a7243fc96a508cbfca8f5d0/ad11b686-5970-42de-9827-91700269875b_v1/current.crl",
    "http://primary-cdn.pki.azure.net/centralus/crls/9a8a2d279a7243fc96a508cbfca8f5d0/7ff044a8-9c28-4529-9d79-76bdb94df99d_v1/current.crl",
  ]
}

variable "crl_refresh_secs" {
  description = "Interval between CRL refresh polls. 3600 (1h) matches Cloud PKI publishing cadence."
  type        = number
  default     = 3600
}

variable "mtls_require_ingest" {
  description = "When true, ingest routes reject requests that arrive without a verified client cert (CMTRACE_MTLS_REQUIRE_INGEST). Set false during pilot to allow the X-Device-Id header fallback while devices roll over to PKCS-issued certs."
  type        = bool
  default     = false
}

variable "crl_fail_open" {
  description = "If true, accept certs whose revocation status is unknown when the CRL CDN is unreachable. Defaults to false (fail-closed). Only enable for air-gapped lab deploys."
  type        = bool
  default     = false
}

# ---------------------------------------------------------------------------
# Postgres
# ---------------------------------------------------------------------------

variable "postgres_sku_name" {
  description = "Postgres flexible-server SKU. \"B_Standard_B1ms\" for pilot (~$15/mo), \"GP_Standard_D2ds_v4\" for prod (~$200/mo)."
  type        = string
  default     = "B_Standard_B1ms"
}

variable "postgres_storage_mb" {
  description = "Postgres storage allocation in MB. Min 32GB, scales in 32GB increments."
  type        = number
  default     = 32768
}

variable "postgres_admin_login" {
  description = "Local Postgres admin login (used only if AAD admin is not yet provisioned — module also configures AAD admin)."
  type        = string
  default     = "cmtraceadmin"
}

variable "postgres_aad_admin_object_id" {
  description = "Object ID of the Entra group/user that becomes the Postgres AAD admin. Should be a group, not a person, so on-call can rotate."
  type        = string
}

# ---------------------------------------------------------------------------
# Container Apps
# ---------------------------------------------------------------------------

variable "aca_use_workload_profile" {
  description = "If true, deploy a workload-profile (D4) ACA env (~$140/mo per replica, predictable latency). If false, use consumption tier (~$30/mo for low traffic, scales to zero)."
  type        = bool
  default     = false
}

variable "aca_min_replicas" {
  description = "Minimum replica count. 1 is recommended even on consumption tier so cold starts don't tank ingest tail latency."
  type        = number
  default     = 1
}

variable "aca_max_replicas" {
  description = "Maximum replica count under autoscale."
  type        = number
  default     = 5
}

variable "aca_cpu" {
  description = "vCPU per replica. 0.5 for pilot, 2.0 for prod."
  type        = number
  default     = 0.5
}

variable "aca_memory" {
  description = "Memory per replica (e.g. \"1Gi\", \"4Gi\"). Must match Azure's allowed CPU/memory pairs."
  type        = string
  default     = "1Gi"
}

variable "log_level" {
  description = "RUST_LOG value for the api-server container."
  type        = string
  default     = "info"
}

# ---------------------------------------------------------------------------
# Application Gateway
# ---------------------------------------------------------------------------

variable "appgw_capacity_min" {
  description = "AppGW v2 autoscale min capacity units. Each unit ~= $0.0072/hr."
  type        = number
  default     = 1
}

variable "appgw_capacity_max" {
  description = "AppGW v2 autoscale max capacity units. 10 is enough for ~5k RPS bursty ingest."
  type        = number
  default     = 10
}

variable "frontend_fqdn" {
  description = "Customer-facing FQDN that DNS will point at the AppGW public IP (e.g. \"api.cmtrace.example.com\"). Module does NOT manage DNS — caller does, after applying."
  type        = string
}

variable "frontend_cert_kv_secret_name" {
  description = "Name of the Key Vault secret holding the public TLS cert (PFX, base64-encoded) the AppGW serves on its frontend listener. Caller uploads via separate process — see runbook §6."
  type        = string
  default     = "appgw-frontend-cert"
}

variable "client_root_ca_kv_secret_name" {
  description = "Name of the Key Vault secret holding the trusted client CA bundle (PEM, base64-encoded). For the live Gell Cloud PKI, this is the Root + Issuing chain concatenated. Caller uploads via separate process — see runbook §6."
  type        = string
  default     = "appgw-client-root-ca"
}

variable "kv_admin_object_id" {
  description = "Object ID of the Entra group that gets Key Vault admin access (cert + secret upload, lifecycle). Module grants the AppGW + ACA managed identities read-only."
  type        = string
}

variable "kv_allow_public_access" {
  description = "If true, KV firewall stays open (network_acls.default_action = Allow). Keep false for production — only AppGW + private endpoints reach KV."
  type        = bool
  default     = false
}
