###############################################################################
# Key Vault for three classes of secret:
#   1. AppGW frontend TLS cert (PFX, base64-encoded)
#   2. AppGW trusted client CA bundle (PEM: Cloud PKI Root + Issuing)
#   3. Postgres admin password (random-generated, consumed by containerapp)
#
# RBAC model: enable_rbac_authorization = true. We avoid legacy access
# policies because (a) they're not role-scopable and (b) azurerm 4.x
# deprecated access_policy nested blocks on the KV resource.
#
# Two managed identities get read:
#   * appgw_identity  — AppGW pulls frontend cert + trusted CA
#   * aca identity    — granted by the containerapp submodule after the
#                       app's system-assigned MI principal ID is known.
#                       (Chicken-and-egg: can't create MI before the app,
#                       can't pin cert secrets before KV — so this submodule
#                       creates the AppGW identity explicitly and exports
#                       both its id + principal_id.)
###############################################################################

resource "azurerm_user_assigned_identity" "appgw" {
  # Re-derive the user-MI name from the AppGW name (which is already the
  # base{-appgw} pattern). Avoids threading a separate "base" through the
  # naming map.
  name                = "${var.naming.appgw}-id"
  location            = var.location
  resource_group_name = var.resource_group_name
  tags                = var.tags
}

resource "azurerm_key_vault" "kv" {
  name                          = var.naming.kv
  location                      = var.location
  resource_group_name           = var.resource_group_name
  tenant_id                     = var.tenant_id
  sku_name                      = "standard"
  purge_protection_enabled      = true
  soft_delete_retention_days    = 7
  rbac_authorization_enabled    = true
  public_network_access_enabled = var.allow_public_access
  tags                          = var.tags

  network_acls {
    default_action = var.allow_public_access ? "Allow" : "Deny"
    bypass         = "AzureServices"
  }
}

# Admin role for the caller-specified group (KV Administrator = full data-plane).
resource "azurerm_role_assignment" "kv_admin" {
  scope                = azurerm_key_vault.kv.id
  role_definition_name = "Key Vault Administrator"
  principal_id         = var.kv_admin_object_id
}

# AppGW MI reads cert + secrets. "Key Vault Secrets User" covers both the
# PFX-as-secret path and the PEM bundle.
resource "azurerm_role_assignment" "kv_appgw_reader" {
  scope                = azurerm_key_vault.kv.id
  role_definition_name = "Key Vault Secrets User"
  principal_id         = azurerm_user_assigned_identity.appgw.principal_id
}

# AppGW also needs certificate-user to bind a cert secret as a listener cert.
resource "azurerm_role_assignment" "kv_appgw_cert_reader" {
  scope                = azurerm_key_vault.kv.id
  role_definition_name = "Key Vault Certificate User"
  principal_id         = azurerm_user_assigned_identity.appgw.principal_id
}

# ---------------------------------------------------------------------------
# Private endpoint — KV is the first thing reachable over the private plane
# so bootstrap secrets (Postgres password, etc.) don't traverse the public
# internet even when allow_public_access is true for operator uploads.
# ---------------------------------------------------------------------------

resource "azurerm_private_dns_zone" "kv" {
  name                = var.naming.kv_pdz
  resource_group_name = var.resource_group_name
  tags                = var.tags
}

resource "azurerm_private_dns_zone_virtual_network_link" "kv" {
  name                  = "${var.naming.kv}-pdzvl"
  resource_group_name   = var.resource_group_name
  private_dns_zone_name = azurerm_private_dns_zone.kv.name
  virtual_network_id    = var.vnet_id
  registration_enabled  = false
}

resource "azurerm_private_endpoint" "kv" {
  name                = "${var.naming.kv}-pe"
  location            = var.location
  resource_group_name = var.resource_group_name
  subnet_id           = var.pe_subnet_id
  tags                = var.tags

  private_service_connection {
    name                           = "${var.naming.kv}-psc"
    private_connection_resource_id = azurerm_key_vault.kv.id
    is_manual_connection           = false
    subresource_names              = ["vault"]
  }

  private_dns_zone_group {
    name                 = "kv-pdzg"
    private_dns_zone_ids = [azurerm_private_dns_zone.kv.id]
  }
}
