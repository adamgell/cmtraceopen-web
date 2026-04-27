###############################################################################
# Resource-naming convention.
#
# Pattern: {prefix}-{env}-{location-short}-{kind}[-{suffix}]
# Example: cmtrace-pilot-cus-aca, cmtrace-prod-eus2-appgw
#
# The location short-codes follow the de-facto Azure community shorthand
# (centralus → cus, eastus2 → eus2, etc.). Anything we haven't mapped falls
# back to the raw region name so naming never silently breaks for a new
# region — the resulting string is just longer.
###############################################################################

locals {
  location_short_map = {
    centralus          = "cus"
    eastus             = "eus"
    eastus2            = "eus2"
    westus             = "wus"
    westus2            = "wus2"
    westus3            = "wus3"
    northcentralus     = "ncus"
    southcentralus     = "scus"
    westcentralus      = "wcus"
    canadacentral      = "cac"
    canadaeast         = "cae"
    westeurope         = "weu"
    northeurope        = "neu"
    uksouth            = "uks"
    ukwest             = "ukw"
    francecentral      = "frc"
    germanywestcentral = "gwc"
    norwayeast         = "noe"
    swedencentral      = "sec"
    switzerlandnorth   = "swn"
    eastasia           = "ea"
    southeastasia      = "sea"
    japaneast          = "jpe"
    japanwest          = "jpw"
    koreacentral       = "krc"
    australiaeast      = "aue"
    australiasoutheast = "ause"
    centralindia       = "cin"
    southindia         = "sin"
    uaenorth           = "uaen"
    brazilsouth        = "brs"
  }

  loc_short = lookup(local.location_short_map, var.location, var.location)

  base = "${var.name_prefix}-${var.environment}-${local.loc_short}"

  # Resource-kind suffixes. Keep keys short; they're part of the rendered name.
  naming = {
    vnet              = "${local.base}-vnet"
    nsg_appgw         = "${local.base}-nsg-appgw"
    nsg_aca           = "${local.base}-nsg-aca"
    nsg_pe            = "${local.base}-nsg-pe"
    pip_appgw         = "${local.base}-pip-appgw"
    appgw             = "${local.base}-appgw"
    waf_policy        = "${local.base}-wafpolicy"
    aca_env           = "${local.base}-acaenv"
    aca_app           = "${local.base}-api"
    law               = "${local.base}-law"
    kv                = substr(replace("${local.base}-kv", "-", ""), 0, 24) # KV name: 3-24, alphanum + dash but no leading/trailing
    kv_dashed         = "${local.base}-kv"                                  # used in tags only, not the resource name
    pg                = "${local.base}-pg"
    pg_db             = "cmtrace"
    pg_pdz            = "${local.base}-pg.private.postgres.database.azure.com"
    storage           = substr(replace(replace("${local.base}sa", "-", ""), "_", ""), 0, 24) # storage: 3-24, lowercase alphanum, no dashes
    storage_container = "bundles"
    storage_pdz       = "privatelink.blob.core.windows.net"
    kv_pdz            = "privatelink.vaultcore.azure.net"
  }

  # ACA env-var inventory. One source of truth — every module that needs to
  # reflect "what env vars does the api-server see" reads from here.
  # Values are (mostly) wired in modules/containerapp/main.tf via for_each.
  api_env_static = {
    CMTRACE_LISTEN_ADDR                = "0.0.0.0:8080"
    CMTRACE_AUTH_MODE                  = "entra"
    CMTRACE_ENTRA_TENANT_ID            = var.entra_tenant_id
    CMTRACE_ENTRA_AUDIENCE             = var.entra_audience
    CMTRACE_ENTRA_JWKS_URI             = "https://login.microsoftonline.com/${var.entra_tenant_id}/discovery/v2.0/keys"
    CMTRACE_BLOB_BACKEND               = "azure"
    CMTRACE_AZURE_STORAGE_CONTAINER    = local.naming.storage_container
    CMTRACE_AZURE_USE_MANAGED_IDENTITY = "true"
    # AppGW terminates TLS; the api-server must NOT also try to terminate.
    CMTRACE_TLS_ENABLED = "false"
    # mTLS env vars — only set when certs_uploaded = true and the CA bundle
    # init container is wired. For pilot with mtls_require_ingest = false,
    # the api-server falls back to X-Device-Id header identity.
    CMTRACE_MTLS_REQUIRE_INGEST = var.mtls_require_ingest ? "true" : "false"
    CMTRACE_CRL_REFRESH_SECS    = tostring(var.crl_refresh_secs)
    CMTRACE_CRL_FAIL_OPEN    = var.crl_fail_open ? "true" : "false"
    CMTRACE_CRL_URLS         = join(",", var.crl_urls)
    CMTRACE_CORS_ORIGINS     = join(",", var.cors_origins)
    CMTRACE_DATA_DIR         = "/var/lib/cmtrace"
    RUST_LOG                 = var.log_level
  }

  default_tags = merge(
    {
      System    = "cmtraceopen"
      ManagedBy = "Terraform"
      Module    = "infra/azure"
    },
    var.tags,
    {
      Environment = var.environment
    },
  )
}
