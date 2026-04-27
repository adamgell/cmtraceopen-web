output "appgw_public_ip" {
  description = "Static public IP of the Application Gateway. Caller points the frontend FQDN A record at this. Null until certs_uploaded = true."
  value       = var.certs_uploaded ? module.appgw[0].public_ip : null
}

output "appgw_public_fqdn" {
  description = "AppGW's *.cloudapp.azure.com convenience name (useful for testing before DNS is cut over). Null until certs_uploaded = true."
  value       = var.certs_uploaded ? module.appgw[0].public_dns_name : null
}

output "ingress_url" {
  description = "Customer-facing HTTPS URL agents and operators connect to."
  value       = "https://${var.frontend_fqdn}"
}

output "container_app_id" {
  description = "Resource ID of the api-server container app."
  value       = module.containerapp.app_id
}

output "container_app_fqdn" {
  description = "Internal ACA ingress FQDN. AppGW backend points here. Should NOT be exposed to end users."
  value       = module.containerapp.app_fqdn
}

output "managed_identity_principal_id" {
  description = "Object ID of the api-server's system-assigned managed identity. Use this when creating downstream RBAC (Postgres AAD groups, additional storage accounts, etc.)."
  value       = module.containerapp.managed_identity_principal_id
}

output "key_vault_id" {
  description = "Key Vault resource ID. Caller uploads the frontend cert + client CA bundle here before applying or before AppGW's first revision rolls."
  value       = module.keyvault.key_vault_id
}

output "key_vault_uri" {
  description = "Key Vault URI (https://<name>.vault.azure.net/) — convenient for `az keyvault secret set --vault-name`."
  value       = module.keyvault.key_vault_uri
}

output "postgres_server_fqdn" {
  description = "FQDN of the Postgres flexible server (private)."
  value       = module.postgres.server_fqdn
}

output "storage_account_name" {
  description = "Storage account name receiving finalized bundles."
  value       = module.storage.storage_account_name
}

output "log_analytics_workspace_id" {
  description = "Log Analytics workspace ID. Consume in caller's monitoring stack."
  value       = azurerm_log_analytics_workspace.law.id
}
