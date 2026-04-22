output "key_vault_id" {
  value = azurerm_key_vault.kv.id
}

output "key_vault_name" {
  value = azurerm_key_vault.kv.name
}

output "key_vault_uri" {
  value = azurerm_key_vault.kv.vault_uri
}

output "appgw_identity_id" {
  description = "User-assigned MI resource ID. Attach to the AppGW so it can pull cert + CA secrets at handshake time."
  value       = azurerm_user_assigned_identity.appgw.id
}

output "appgw_identity_principal_id" {
  description = "Object ID of the AppGW MI. Useful when caller needs to grant additional KV roles."
  value       = azurerm_user_assigned_identity.appgw.principal_id
}

output "appgw_identity_client_id" {
  value = azurerm_user_assigned_identity.appgw.client_id
}
