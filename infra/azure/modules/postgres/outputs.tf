output "server_id" {
  value = azurerm_postgresql_flexible_server.pg.id
}

output "server_fqdn" {
  value = azurerm_postgresql_flexible_server.pg.fqdn
}

output "database_name" {
  value = azurerm_postgresql_flexible_server_database.db.name
}

output "connection_string_secret_id" {
  description = "KV secret resource ID holding CMTRACE_DATABASE_URL. ACA references this via `secretRef` for env-from-keyvault binding."
  value       = azurerm_key_vault_secret.pg_connection_string.id
}

output "connection_string_secret_name" {
  value = azurerm_key_vault_secret.pg_connection_string.name
}
