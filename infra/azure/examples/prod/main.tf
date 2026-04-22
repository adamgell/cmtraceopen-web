###############################################################################
# Example caller — drops the cmtraceopen Azure module into a prod-shaped
# Terraform stack. Copy into your existing repo, point `source` at wherever
# you've vendored the module (git ref or local path), and `terraform apply`.
###############################################################################

terraform {
  required_version = ">= 1.6.0"
  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 4.0"
    }
    azapi = {
      source  = "Azure/azapi"
      version = "~> 2.0"
    }
  }
}

provider "azurerm" {
  features {
    key_vault {
      # Required for safe `terraform destroy` against KV with purge protection.
      purge_soft_deleted_secrets_on_destroy      = false
      purge_soft_deleted_certificates_on_destroy = false
    }
  }
}

provider "azapi" {}

# RG is managed outside this module so on-call can scope cleanup carefully.
resource "azurerm_resource_group" "rg" {
  name     = "rg-cmtraceopen-prod-cus"
  location = "centralus"
  tags = {
    System     = "cmtraceopen"
    ManagedBy  = "Terraform"
    Owner      = "platform-eng"
    CostCenter = "1234"
  }
}

module "cmtrace_api" {
  source = "../../"

  environment         = "prod"
  location            = "centralus"
  resource_group_name = azurerm_resource_group.rg.name
  tags                = azurerm_resource_group.rg.tags

  # ----- Container image -----
  image = "ghcr.io/adamgell/cmtraceopen-api:v0.1.0"

  # ----- Networking -----
  # The defaults give 10.50.0.0/16; if your org uses a different RFC1918 plan,
  # override here so AppGW + ACA + PE subnets fit inside your hub/spoke.
  vnet_address_space   = ["10.50.0.0/16"]
  appgw_subnet_cidr    = "10.50.0.0/24"
  aca_subnet_cidr      = "10.50.2.0/23"
  postgres_subnet_cidr = "10.50.4.0/24"
  pe_subnet_cidr       = "10.50.5.0/24"

  # ----- Auth -----
  entra_tenant_id = var.entra_tenant_id
  entra_audience  = var.entra_audience
  cors_origins    = ["https://cmtrace.example.com"]

  # ----- Postgres -----
  # GP_Standard_D2ds_v4 = ~$200/mo; bump to D4ds_v5 if pilot data shows
  # ingest-finalize commit time creeping past 50ms p95.
  postgres_sku_name            = "GP_Standard_D2ds_v4"
  postgres_storage_mb          = 65536 # 64GB; flex auto-grows in 32GB increments
  postgres_aad_admin_object_id = var.pg_aad_admin_group_object_id

  # ----- ACA sizing -----
  # Workload-profile gives consistent latency for the operator query path.
  # Consumption tier is fine for ingest spikes (it's the bulk of traffic).
  aca_use_workload_profile = true
  aca_min_replicas         = 2
  aca_max_replicas         = 8
  aca_cpu                  = 2.0
  aca_memory               = "4Gi"

  # ----- AppGW -----
  appgw_capacity_min            = 2
  appgw_capacity_max            = 10
  frontend_fqdn                 = "api.cmtrace.example.com"
  frontend_cert_kv_secret_name  = "appgw-frontend-cert"
  client_root_ca_kv_secret_name = "appgw-client-root-ca"

  # ----- Key Vault -----
  kv_admin_object_id     = var.kv_admin_group_object_id
  kv_allow_public_access = false
}

# ---------------------------------------------------------------------------
# Variables for sensitive identifiers — keep these in tfvars or your
# CI's secret store, NOT in version control.
# ---------------------------------------------------------------------------

variable "entra_tenant_id" {
  type = string
}

variable "entra_audience" {
  type = string
}

variable "kv_admin_group_object_id" {
  description = "Entra group object ID with KV admin (cert/secret upload)."
  type        = string
}

variable "pg_aad_admin_group_object_id" {
  description = "Entra group object ID that becomes Postgres AAD admin."
  type        = string
}

# ---------------------------------------------------------------------------
# Surface the URL + IP back to the caller's stack for DNS automation.
# ---------------------------------------------------------------------------

output "appgw_public_ip" {
  value = module.cmtrace_api.appgw_public_ip
}

output "ingress_url" {
  value = module.cmtrace_api.ingress_url
}

output "key_vault_uri" {
  value = module.cmtrace_api.key_vault_uri
}
