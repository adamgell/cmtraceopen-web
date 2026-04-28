###############################################################################
# Pilot environment — simplified. Just ACA (external) + Postgres.
# No AppGW, no KV, no mTLS, no VNet complexity.
# Cloudflare handles TLS termination at the edge.
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
  }
}

provider "azurerm" {
  features {}
}

provider "azuread" {}

data "azurerm_client_config" "current" {}

# ---------------------------------------------------------------------------
# Resource Group
# ---------------------------------------------------------------------------

resource "azurerm_resource_group" "pilot" {
  name     = "rg-cmtrace-pilot"
  location = "centralus"
  tags = {
    System      = "cmtraceopen"
    Environment = "pilot"
    ManagedBy   = "Terraform"
  }
}

# ---------------------------------------------------------------------------
# Log Analytics (required by ACA)
# ---------------------------------------------------------------------------

resource "azurerm_log_analytics_workspace" "law" {
  name                = "cmtrace-pilot-law"
  location            = azurerm_resource_group.pilot.location
  resource_group_name = azurerm_resource_group.pilot.name
  sku                 = "PerGB2018"
  retention_in_days   = 30
  tags                = azurerm_resource_group.pilot.tags
}

# ---------------------------------------------------------------------------
# Postgres Flexible Server (public access for pilot simplicity)
# ---------------------------------------------------------------------------

resource "random_password" "pg" {
  length  = 32
  special = true
}

resource "azurerm_postgresql_flexible_server" "pg" {
  name                          = "cmtrace-pilot-pg"
  resource_group_name           = azurerm_resource_group.pilot.name
  location                      = azurerm_resource_group.pilot.location
  version                       = "16"
  administrator_login           = "cmtraceadmin"
  administrator_password        = random_password.pg.result
  sku_name                      = "B_Standard_B1ms"
  storage_mb                    = 32768
  zone                          = "1"
  public_network_access_enabled = true
  tags                          = azurerm_resource_group.pilot.tags

  authentication {
    active_directory_auth_enabled = false
    password_auth_enabled         = true
  }
}

resource "azurerm_postgresql_flexible_server_database" "db" {
  name      = "cmtrace"
  server_id = azurerm_postgresql_flexible_server.pg.id
  charset   = "UTF8"
  collation = "en_US.utf8"
}

resource "azurerm_postgresql_flexible_server_firewall_rule" "allow_azure" {
  name             = "AllowAzureServices"
  server_id        = azurerm_postgresql_flexible_server.pg.id
  start_ip_address = "0.0.0.0"
  end_ip_address   = "0.0.0.0"
}

# ---------------------------------------------------------------------------
# Storage Account (for blob bundles)
# ---------------------------------------------------------------------------

resource "azurerm_storage_account" "sa" {
  name                     = "cmtracepilotsa"
  resource_group_name      = azurerm_resource_group.pilot.name
  location                 = azurerm_resource_group.pilot.location
  account_tier             = "Standard"
  account_replication_type = "LRS"
  tags                     = azurerm_resource_group.pilot.tags
}

resource "azurerm_storage_container" "bundles" {
  name                  = "bundles"
  storage_account_id    = azurerm_storage_account.sa.id
  container_access_type = "private"
}

# ---------------------------------------------------------------------------
# Container Apps Environment + App (external ingress)
# ---------------------------------------------------------------------------

resource "azurerm_container_app_environment" "env" {
  name                       = "cmtrace-pilot-env"
  location                   = azurerm_resource_group.pilot.location
  resource_group_name        = azurerm_resource_group.pilot.name
  log_analytics_workspace_id = azurerm_log_analytics_workspace.law.id
  tags                       = azurerm_resource_group.pilot.tags
}

resource "azurerm_container_app" "api" {
  name                         = "cmtrace-pilot-api"
  container_app_environment_id = azurerm_container_app_environment.env.id
  resource_group_name          = azurerm_resource_group.pilot.name
  revision_mode                = "Single"
  tags                         = azurerm_resource_group.pilot.tags

  identity {
    type = "SystemAssigned"
  }

  ingress {
    external_enabled = true
    target_port      = 8080
    transport        = "auto"
    traffic_weight {
      latest_revision = true
      percentage      = 100
    }
  }

  template {
    min_replicas = 1
    max_replicas = 3

    volume {
      name         = "data-dir"
      storage_type = "EmptyDir"
    }

    container {
      name   = "api"
      image  = var.api_image
      cpu    = 0.5
      memory = "1Gi"

      env {
        name  = "CMTRACE_LISTEN_ADDR"
        value = "0.0.0.0:8080"
      }
      env {
        name  = "CMTRACE_AUTH_MODE"
        value = "entra"
      }
      env {
        name  = "CMTRACE_ENTRA_TENANT_ID"
        value = var.entra_tenant_id
      }
      env {
        name  = "CMTRACE_ENTRA_AUDIENCE"
        value = var.entra_audience
      }
      env {
        name  = "CMTRACE_ENTRA_JWKS_URI"
        value = "https://login.microsoftonline.com/${var.entra_tenant_id}/discovery/v2.0/keys"
      }
      env {
        name  = "CMTRACE_DATABASE_URL"
        value = "postgres://cmtraceadmin:${urlencode(random_password.pg.result)}@${azurerm_postgresql_flexible_server.pg.fqdn}:5432/cmtrace?sslmode=require"
      }
      env {
        name  = "CMTRACE_BLOB_BACKEND"
        value = "azure"
      }
      env {
        name  = "CMTRACE_AZURE_STORAGE_ACCOUNT"
        value = azurerm_storage_account.sa.name
      }
      env {
        name  = "CMTRACE_AZURE_STORAGE_CONTAINER"
        value = "bundles"
      }
      env {
        name  = "CMTRACE_AZURE_USE_MANAGED_IDENTITY"
        value = "true"
      }
      env {
        name  = "CMTRACE_TLS_ENABLED"
        value = "false"
      }
      env {
        name  = "CMTRACE_MTLS_REQUIRE_INGEST"
        value = "false"
      }
      env {
        name  = "CMTRACE_DEV_UNAUTH_READS"
        value = "1"
      }
      env {
        name  = "CMTRACE_DATA_DIR"
        value = "/var/lib/cmtrace"
      }
      env {
        name  = "RUST_LOG"
        value = "info"
      }

      volume_mounts {
        name = "data-dir"
        path = "/var/lib/cmtrace"
      }

      liveness_probe {
        transport               = "HTTP"
        port                    = 8080
        path                    = "/healthz"
        initial_delay           = 10
        interval_seconds        = 30
        failure_count_threshold = 3
      }

      readiness_probe {
        transport               = "HTTP"
        port                    = 8080
        path                    = "/readyz"
        interval_seconds        = 10
        failure_count_threshold = 3
      }
    }
  }
}

# RBAC: ACA MI -> Storage Blob Data Contributor
resource "azurerm_role_assignment" "blob_writer" {
  scope                = azurerm_storage_account.sa.id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = azurerm_container_app.api.identity[0].principal_id
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

variable "api_image" {
  type    = string
  default = "ghcr.io/adamgell/cmtraceopen-api:0.1.0"
}

# ---------------------------------------------------------------------------
# Outputs
# ---------------------------------------------------------------------------

output "api_url" {
  value = "https://${azurerm_container_app.api.ingress[0].fqdn}"
}

output "api_fqdn" {
  description = "Point pilot.cmtrace.net CNAME at this"
  value       = azurerm_container_app.api.ingress[0].fqdn
}

output "postgres_fqdn" {
  value     = azurerm_postgresql_flexible_server.pg.fqdn
  sensitive = true
}
