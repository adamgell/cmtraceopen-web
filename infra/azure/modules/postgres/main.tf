###############################################################################
# Postgres flexible server, VNet-injected, with both AAD admin (preferred)
# and a generated local-login password (kept for break-glass + initial bring-up
# before AAD-only mode is flipped on).
#
# The api-server connects via DATABASE_URL with `sslmode=require`. When the
# api-server is updated to use AAD token auth (TODO — needs `azure-identity`
# crate wiring), we'll flip `password_auth_enabled = false` and the secret
# becomes purely break-glass.
###############################################################################

resource "random_password" "pg_admin" {
  length           = 32
  special          = true
  min_special      = 2
  override_special = "_-." # keep URL-safe
}

resource "azurerm_private_dns_zone" "pg" {
  name                = var.naming.pg_pdz
  resource_group_name = var.resource_group_name
  tags                = var.tags
}

resource "azurerm_private_dns_zone_virtual_network_link" "pg" {
  name                  = "${var.naming.pg}-pdzvl"
  resource_group_name   = var.resource_group_name
  private_dns_zone_name = azurerm_private_dns_zone.pg.name
  virtual_network_id    = var.vnet_id
  registration_enabled  = false
}

resource "azurerm_postgresql_flexible_server" "pg" {
  name                          = var.naming.pg
  resource_group_name           = var.resource_group_name
  location                      = var.location
  version                       = "16"
  delegated_subnet_id           = var.postgres_subnet_id
  private_dns_zone_id           = azurerm_private_dns_zone.pg.id
  administrator_login           = var.admin_login
  administrator_password        = random_password.pg_admin.result
  zone                          = "1"
  storage_mb                    = var.storage_mb
  sku_name                      = var.sku_name
  # Retain automated backups for 30 days (maximum for Flexible Server).
  # Geo-redundant backup replicates backups to the Azure paired region,
  # providing a recovery point even if the primary region is unavailable.
  # See ops/postgres/RUNBOOK.md for the restore flow.
  backup_retention_days         = 30
  geo_redundant_backup_enabled  = true
  public_network_access_enabled = false
  tags                          = var.tags

  authentication {
    active_directory_auth_enabled = true
    password_auth_enabled         = true
    tenant_id                     = var.aad_admin_tenant_id
  }

  lifecycle {
    # Keep apply runs from oscillating when Azure pushes minor patch versions.
    ignore_changes = [zone]
  }
}

resource "azurerm_postgresql_flexible_server_database" "db" {
  name      = var.naming.pg_db
  server_id = azurerm_postgresql_flexible_server.pg.id
  charset   = "UTF8"
  collation = "en_US.utf8"
}

resource "azurerm_postgresql_flexible_server_active_directory_administrator" "aad" {
  server_name         = azurerm_postgresql_flexible_server.pg.name
  resource_group_name = var.resource_group_name
  tenant_id           = var.aad_admin_tenant_id
  object_id           = var.aad_admin_object_id
  principal_name      = "cmtrace-pg-aad-admin"
  principal_type      = "Group"
}

# Wire SSL on. Postgres-flex defaults to require_secure_transport=on but be
# explicit so a future operator who flips it sees the diff.
resource "azurerm_postgresql_flexible_server_configuration" "ssl" {
  name      = "require_secure_transport"
  server_id = azurerm_postgresql_flexible_server.pg.id
  value     = "ON"
}

# ---------------------------------------------------------------------------
# Diagnostic settings — surface query/error logs to LAW.
# ---------------------------------------------------------------------------

resource "azurerm_monitor_diagnostic_setting" "pg" {
  name                       = "${var.naming.pg}-diag"
  target_resource_id         = azurerm_postgresql_flexible_server.pg.id
  log_analytics_workspace_id = var.log_analytics_id

  enabled_log {
    category = "PostgreSQLLogs"
  }

  enabled_metric {
    category = "AllMetrics"
  }
}

# ---------------------------------------------------------------------------
# Persist the generated admin password + the assembled connection string as
# KV secrets so the containerapp module can reference them at deploy time.
#
# Conn string format matches what the api-server expects in
# CMTRACE_DATABASE_URL (and what sqlx parses):
#   postgres://<user>:<pass>@<host>:5432/<db>?sslmode=require
# ---------------------------------------------------------------------------

resource "azurerm_key_vault_secret" "pg_password" {
  name         = "postgres-admin-password"
  value        = random_password.pg_admin.result
  key_vault_id = var.key_vault_id
  content_type = "Postgres admin password (random-generated)"
}

resource "azurerm_key_vault_secret" "pg_connection_string" {
  name         = "cmtrace-database-url"
  value        = "postgres://${var.admin_login}:${random_password.pg_admin.result}@${azurerm_postgresql_flexible_server.pg.fqdn}:5432/${var.naming.pg_db}?sslmode=require"
  key_vault_id = var.key_vault_id
  content_type = "CMTRACE_DATABASE_URL"

  depends_on = [
    azurerm_postgresql_flexible_server_database.db,
  ]
}

# Encryption at rest: Flexible Server encrypts with platform-managed
# keys (AES-256) by default. To use a customer-managed key (CMK) instead,
# add a `customer_managed_key { ... }` block on the server resource above
# and supply a Key Vault key.
# See: https://learn.microsoft.com/azure/postgresql/flexible-server/concepts-data-encryption
