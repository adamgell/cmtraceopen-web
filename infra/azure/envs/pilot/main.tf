###############################################################################
# Pilot environment — creates the RG, Entra groups, and calls the module.
#
# Two-phase apply:
#   1. `terraform apply` — creates RG, KV, network, Postgres, ACA, AppGW.
#      AppGW will be unhealthy until certs are uploaded.
#   2. Upload certs to KV (see outputs for commands), then apply again to
#      let AppGW pick them up.
###############################################################################

terraform {
  required_version = ">= 1.6.0"
  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 4.0"
    }
    azuread = {
      source  = "hashicorp/azuread"
      version = "~> 3.0"
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
      purge_soft_deleted_secrets_on_destroy      = false
      purge_soft_deleted_certificates_on_destroy = false
    }
  }
}

provider "azuread" {}
provider "azapi" {}

data "azurerm_client_config" "current" {}

# ---------------------------------------------------------------------------
# Resource Group
# ---------------------------------------------------------------------------

resource "azurerm_resource_group" "pilot" {
  name     = "rg-cmtraceopen-pilot-cus"
  location = "centralus"
  tags = {
    System      = "cmtraceopen"
    Environment = "pilot"
    ManagedBy   = "Terraform"
  }
}

# ---------------------------------------------------------------------------
# Entra Security Groups
# ---------------------------------------------------------------------------

resource "azuread_group" "kv_admins" {
  display_name     = "cmtraceopen-pilot-kv-admins"
  security_enabled = true
  description      = "Key Vault admin access for cmtraceopen pilot (cert + secret upload)"
  members          = [data.azurerm_client_config.current.object_id]
}

resource "azuread_group" "pg_admins" {
  display_name     = "cmtraceopen-pilot-pg-admins"
  security_enabled = true
  description      = "Postgres AAD admin for cmtraceopen pilot"
  members          = [data.azurerm_client_config.current.object_id]
}

# ---------------------------------------------------------------------------
# CMTrace API Module
# ---------------------------------------------------------------------------

module "cmtrace_api" {
  source = "../../"

  environment         = "pilot"
  location            = "centralus"
  resource_group_name = azurerm_resource_group.pilot.name
  tags                = azurerm_resource_group.pilot.tags

  image = var.api_image

  entra_tenant_id = var.entra_tenant_id
  entra_audience  = var.entra_audience
  cors_origins    = var.cors_origins

  postgres_sku_name            = "B_Standard_B1ms"
  postgres_storage_mb          = 32768
  postgres_aad_admin_object_id = azuread_group.pg_admins.object_id

  aca_use_workload_profile = false
  aca_min_replicas         = 1
  aca_max_replicas         = 3
  aca_cpu                  = 0.5
  aca_memory               = "1Gi"

  appgw_capacity_min = 1
  appgw_capacity_max = 3
  frontend_fqdn      = var.frontend_fqdn

  kv_admin_object_id     = azuread_group.kv_admins.object_id
  kv_allow_public_access = true

  mtls_require_ingest = false
}

# ---------------------------------------------------------------------------
# Variables
# ---------------------------------------------------------------------------

variable "entra_tenant_id" {
  type    = string
  default = "00c171a4-0053-4cae-ab80-0bd18db2e0fc"
}

variable "entra_audience" {
  type    = string
  default = "b2990298-7cdd-4426-b311-2df0221b6eca"
}

variable "frontend_fqdn" {
  description = "Public FQDN for the pilot API (point DNS A record at the AppGW IP after apply)."
  type        = string
}

variable "api_image" {
  type    = string
  default = "ghcr.io/adamgell/cmtraceopen-api:v0.1.0"
}

variable "cors_origins" {
  type    = list(string)
  default = []
}

# ---------------------------------------------------------------------------
# Outputs
# ---------------------------------------------------------------------------

output "appgw_public_ip" {
  description = "Point your DNS A record here."
  value       = module.cmtrace_api.appgw_public_ip
}

output "appgw_test_fqdn" {
  description = "Azure-assigned *.cloudapp.azure.com name (works before DNS cutover)."
  value       = module.cmtrace_api.appgw_public_fqdn
}

output "ingress_url" {
  value = module.cmtrace_api.ingress_url
}

output "key_vault_uri" {
  value = module.cmtrace_api.key_vault_uri
}

output "key_vault_id" {
  value = module.cmtrace_api.key_vault_id
}

output "cert_upload_commands" {
  description = "Run these after the first apply to upload certs to KV."
  value       = <<-EOT
    # 1. Upload the frontend TLS cert (PFX with private key):
    az keyvault secret set \
      --vault-name $(terraform output -raw key_vault_uri | sed 's|https://||;s|.vault.*||') \
      --name appgw-frontend-cert \
      --file /path/to/frontend-cert.pfx \
      --encoding base64

    # 2. Upload the Cloud PKI CA bundle (Root + Issuing PEM concatenated):
    cat gell-pki-root.pem gell-pki-issuing.pem > ca-bundle.pem
    az keyvault secret set \
      --vault-name $(terraform output -raw key_vault_uri | sed 's|https://||;s|.vault.*||') \
      --name appgw-client-root-ca \
      --file ca-bundle.pem \
      --encoding utf-8

    # 3. Re-run terraform apply so AppGW picks up the certs.
  EOT
}
