output "app_id" {
  value = azurerm_container_app.api.id
}

output "app_fqdn" {
  description = "ACA-assigned ingress FQDN. Internal-only since `external_enabled = false`. AppGW backend points here."
  value       = azurerm_container_app.api.ingress[0].fqdn
}

output "managed_identity_principal_id" {
  value = azurerm_container_app.api.identity[0].principal_id
}

output "managed_identity_tenant_id" {
  value = azurerm_container_app.api.identity[0].tenant_id
}

output "environment_id" {
  value = azurerm_container_app_environment.env.id
}

output "environment_name" {
  value = azurerm_container_app_environment.env.name
}
