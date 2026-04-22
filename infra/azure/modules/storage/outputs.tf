output "storage_account_id" {
  value = azurerm_storage_account.sa.id
}

output "storage_account_name" {
  value = azurerm_storage_account.sa.name
}

output "container_name" {
  value = azurerm_storage_container.bundles.name
}
