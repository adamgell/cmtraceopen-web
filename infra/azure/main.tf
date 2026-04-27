###############################################################################
# Module entrypoint — orchestrates the six submodules in the order Azure
# requires (network → KV → DB/storage → ACA → AppGW). Anything cross-cutting
# lives here; submodules are leaf-level so they're independently testable.
#
# Submodule wiring rules:
#   * Submodules NEVER read from `var.*` directly — they take only what they
#     need as inputs. This keeps `terraform plan` graphs minimal and makes
#     it possible to swap one submodule for a caller-provided alternative
#     (e.g. a brownfield Postgres) by replacing the `module "postgres"` call
#     with a `data` block + null wiring.
#   * The naming convention from `locals.tf` is computed once here and
#     passed in. Submodules don't reconstruct names.
###############################################################################

data "azurerm_client_config" "current" {}

# ---------------------------------------------------------------------------
# Diagnostic sink — Container Apps env writes here; AppGW + KV + Postgres
# also stream their diagnostic settings here for a single pane of glass.
# ---------------------------------------------------------------------------

resource "azurerm_log_analytics_workspace" "law" {
  name                = local.naming.law
  location            = var.location
  resource_group_name = var.resource_group_name
  sku                 = "PerGB2018"
  retention_in_days   = 30
  tags                = local.default_tags
}

# ---------------------------------------------------------------------------
# Submodules
# ---------------------------------------------------------------------------

module "network" {
  source = "./modules/network"

  resource_group_name = var.resource_group_name
  location            = var.location
  tags                = local.default_tags
  naming              = local.naming

  vnet_address_space   = var.vnet_address_space
  appgw_subnet_cidr    = var.appgw_subnet_cidr
  aca_subnet_cidr      = var.aca_subnet_cidr
  postgres_subnet_cidr = var.postgres_subnet_cidr
  pe_subnet_cidr       = var.pe_subnet_cidr
}

module "keyvault" {
  source = "./modules/keyvault"

  resource_group_name = var.resource_group_name
  location            = var.location
  tags                = local.default_tags
  naming              = local.naming

  tenant_id           = data.azurerm_client_config.current.tenant_id
  kv_admin_object_id  = var.kv_admin_object_id
  allow_public_access = var.kv_allow_public_access

  pe_subnet_id = module.network.pe_subnet_id
  vnet_id      = module.network.vnet_id
}

module "postgres" {
  source = "./modules/postgres"

  resource_group_name = var.resource_group_name
  location            = var.location
  tags                = local.default_tags
  naming              = local.naming

  postgres_subnet_id  = module.network.postgres_subnet_id
  vnet_id             = module.network.vnet_id
  sku_name            = var.postgres_sku_name
  storage_mb          = var.postgres_storage_mb
  admin_login         = var.postgres_admin_login
  aad_admin_object_id = var.postgres_aad_admin_object_id
  aad_admin_tenant_id = data.azurerm_client_config.current.tenant_id
  log_analytics_id    = azurerm_log_analytics_workspace.law.id
  key_vault_id        = module.keyvault.key_vault_id
}

module "storage" {
  source = "./modules/storage"

  resource_group_name = var.resource_group_name
  location            = var.location
  tags                = local.default_tags
  naming              = local.naming

  pe_subnet_id     = module.network.pe_subnet_id
  vnet_id          = module.network.vnet_id
  log_analytics_id = azurerm_log_analytics_workspace.law.id
}

# Operator-uploaded CA bundle secret (Root + Issuing CA PEM). Referenced by
# the containerapp init container to write /var/lib/cmtrace/ca-bundle.pem.
# The operator uploads this before `terraform apply` — see runbook §6.
data "azurerm_key_vault_secret" "client_ca_bundle" {
  name         = var.client_root_ca_kv_secret_name
  key_vault_id = module.keyvault.key_vault_id
}

module "containerapp" {
  source = "./modules/containerapp"

  resource_group_name = var.resource_group_name
  location            = var.location
  tags                = local.default_tags
  naming              = local.naming

  aca_subnet_id        = module.network.aca_subnet_id
  log_analytics_id     = azurerm_log_analytics_workspace.law.id
  log_analytics_key_id = azurerm_log_analytics_workspace.law.workspace_id

  image                  = var.image
  image_pull_secret_name = var.image_pull_secret_name
  use_workload_profile   = var.aca_use_workload_profile
  min_replicas           = var.aca_min_replicas
  max_replicas           = var.aca_max_replicas
  cpu                    = var.aca_cpu
  memory                 = var.aca_memory

  static_env_vars = local.api_env_static

  # Secrets pulled from KV at container start via secret-store-csi-style refs.
  # The submodule wires these as ACA secrets backed by KV references.
  key_vault_id           = module.keyvault.key_vault_id
  key_vault_uri          = module.keyvault.key_vault_uri
  postgres_url_secret_id = module.postgres.connection_string_secret_id
  storage_account_name   = module.storage.storage_account_name
  storage_account_id     = module.storage.storage_account_id

  postgres_server_fqdn   = module.postgres.server_fqdn
  postgres_database_name = module.postgres.database_name

  client_ca_bundle_secret_id = data.azurerm_key_vault_secret.client_ca_bundle.versionless_id
}

module "appgw" {
  source = "./modules/appgw"

  resource_group_name = var.resource_group_name
  location            = var.location
  tags                = local.default_tags
  naming              = local.naming

  appgw_subnet_id  = module.network.appgw_subnet_id
  log_analytics_id = azurerm_log_analytics_workspace.law.id

  capacity_min  = var.appgw_capacity_min
  capacity_max  = var.appgw_capacity_max
  frontend_fqdn = var.frontend_fqdn

  # Backend = the ACA app's ingress FQDN.
  backend_fqdn = module.containerapp.app_fqdn

  # Cert + trusted CA come from KV. Both secrets are uploaded out-of-band
  # by the caller before `apply` (see runbook §6).
  key_vault_id                = module.keyvault.key_vault_id
  frontend_cert_secret_name   = var.frontend_cert_kv_secret_name
  client_root_ca_secret_name  = var.client_root_ca_kv_secret_name
  appgw_identity_principal_id = module.keyvault.appgw_identity_principal_id
  appgw_identity_id           = module.keyvault.appgw_identity_id
}
