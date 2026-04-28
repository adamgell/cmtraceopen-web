###############################################################################
# Container Apps environment + the cmtraceopen-api app.
#
# Why azapi for the env (and partially the app):
#   * `azurerm_container_app_environment` 4.x supports workload profiles via
#     `workload_profile {}` blocks but its zoneRedundant + infrastructureResourceGroup
#     plumbing has rough edges that surface as forced-replace diffs on
#     unrelated changes. azapi gives us first-party API stability.
#   * `azurerm_container_app` itself is mature enough — we use it for the
#     app definition. The one feature it doesn't yet expose cleanly is
#     "secret-from-keyvault by URI with a user-MI" (preview); we use the
#     system-assigned identity + KV-uri secret reference syntax that
#     azurerm 4.x added in 4.5+.
#
# Identity strategy:
#   System-assigned MI on the app. Used for:
#     * Pulling KV secrets at container start (via the KV secret reference
#       binding on each ACA secret).
#     * Authenticating to the storage account for blob writes.
#     * (Future) AAD token auth to Postgres once the api-server gains
#       azure-identity wiring.
###############################################################################

# ---------------------------------------------------------------------------
# Environment (LAW-attached, optional workload profile)
# ---------------------------------------------------------------------------

resource "azurerm_container_app_environment" "env" {
  name                           = var.naming.aca_env
  location                       = var.location
  resource_group_name            = var.resource_group_name
  log_analytics_workspace_id     = var.log_analytics_id
  infrastructure_subnet_id       = var.aca_subnet_id
  internal_load_balancer_enabled = true # ingress only reachable from the VNet (AppGW)
  tags                           = var.tags

  workload_profile {
    name                  = "Consumption"
    workload_profile_type = "Consumption"
  }

  dynamic "workload_profile" {
    for_each = var.use_workload_profile ? [1] : []
    content {
      name                  = "wp-d4"
      workload_profile_type = "D4"
      minimum_count         = 1
      maximum_count         = 3
    }
  }
}

# ---------------------------------------------------------------------------
# Container app — the api-server itself.
# ---------------------------------------------------------------------------

resource "azurerm_container_app" "api" {
  name                         = var.naming.aca_app
  container_app_environment_id = azurerm_container_app_environment.env.id
  resource_group_name          = var.resource_group_name
  revision_mode                = "Single"
  workload_profile_name        = var.use_workload_profile ? "wp-d4" : "Consumption"
  tags                         = var.tags

  identity {
    type = "SystemAssigned"
  }

  # KV-backed secret: CMTRACE_DATABASE_URL
  # This binds the ACA secret to the KV secret URI; the platform pulls
  # the live value at container start using the system-assigned MI.
  secret {
    name                = "cmtrace-database-url"
    key_vault_secret_id = var.postgres_url_secret_id
    identity            = "System"
  }


  ingress {
    external_enabled = false # AppGW reaches us internally; no public ingress
    target_port      = 8080
    transport        = "http" # AppGW->ACA over HTTP inside the VNet; fine because both ends are inside Azure backbone + the VNet ACL
    traffic_weight {
      latest_revision = true
      percentage      = 100
    }
  }

  template {
    min_replicas = var.min_replicas
    max_replicas = var.max_replicas

    volume {
      name         = "data-dir"
      storage_type = "EmptyDir"
    }

    container {
      name   = "api"
      image  = var.image
      cpu    = var.cpu
      memory = var.memory

      # Static env vars from the parent's locals.api_env_static map.
      # for_each over the map so adding a new env var is one edit in
      # locals.tf, not a round-trip through this submodule.
      dynamic "env" {
        for_each = var.static_env_vars
        content {
          name  = env.key
          value = env.value
        }
      }

      # Secret-backed env: CMTRACE_DATABASE_URL
      env {
        name        = "CMTRACE_DATABASE_URL"
        secret_name = "cmtrace-database-url"
      }

      # Storage account name has to be a non-secret env, sourced from the
      # storage submodule's output — set explicitly here rather than via
      # the static map so the dependency graph is obvious.
      env {
        name  = "CMTRACE_AZURE_STORAGE_ACCOUNT"
        value = var.storage_account_name
      }

      volume_mounts {
        name = "data-dir"
        path = "/var/lib/cmtrace"
      }

      liveness_probe {
        transport               = "HTTP"
        port                    = 8080
        path                    = "/healthz"
        initial_delay           = 5
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

    # HTTP-RPS scale rule. Tuned for our ingest workload (lots of small
    # PUT chunks): scale on average concurrent requests rather than CPU.
    http_scale_rule {
      name                = "http-rps"
      concurrent_requests = "50"
    }
  }
}

# ---------------------------------------------------------------------------
# RBAC: api-server's MI -> Storage Blob Data Contributor on the storage
# account. Needed for the object_store crate to PUT/GET blobs without a
# shared key.
# ---------------------------------------------------------------------------

resource "azurerm_role_assignment" "blob_writer" {
  scope                = var.storage_account_id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = azurerm_container_app.api.identity[0].principal_id
}

# ---------------------------------------------------------------------------
# RBAC: api-server's MI -> Key Vault Secrets User on the KV.
# Required so the ACA platform can resolve the CMTRACE_DATABASE_URL secret
# reference at container start.
# ---------------------------------------------------------------------------

resource "azurerm_role_assignment" "kv_reader" {
  scope                = var.key_vault_id
  role_definition_name = "Key Vault Secrets User"
  principal_id         = azurerm_container_app.api.identity[0].principal_id
}
