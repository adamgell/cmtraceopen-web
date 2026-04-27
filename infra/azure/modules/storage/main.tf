###############################################################################
# Storage account for finalized bundles.
#
# Choices:
#   * Standard_LRS — bundles are dev/operator-recoverable from devices in
#     the worst case; cross-zone redundancy isn't worth the cost premium
#     for a pilot. Production should consider _ZRS.
#   * Hierarchical namespace OFF — object_store crate uses the blob API,
#     not the ADLS Gen2 API, so HNS adds nothing.
#   * Public access disabled at the account level. ACA reaches the account
#     via the private endpoint over the managed identity binding.
#   * Lifecycle policy promotes blobs to Cool after 30d and deletes after
#     90d. Tweak in caller code if retention needs change.
###############################################################################

resource "azurerm_storage_account" "sa" {
  name                            = var.naming.storage
  resource_group_name             = var.resource_group_name
  location                        = var.location
  account_tier                    = "Standard"
  account_replication_type        = "LRS"
  account_kind                    = "StorageV2"
  https_traffic_only_enabled      = true
  min_tls_version                 = "TLS1_2"
  allow_nested_items_to_be_public = false
  shared_access_key_enabled       = true  # Terraform needs key auth to create containers; ACA uses MI at runtime
  public_network_access_enabled   = true  # Terraform needs data-plane access during apply; lock down post-deploy
  default_to_oauth_authentication = true
  tags                            = var.tags

  network_rules {
    default_action = "Deny"
    bypass         = ["AzureServices"]
  }

  blob_properties {
    delete_retention_policy {
      days = 7
    }
    container_delete_retention_policy {
      days = 7
    }
  }
}

resource "azurerm_storage_container" "bundles" {
  name                  = var.naming.storage_container
  storage_account_id    = azurerm_storage_account.sa.id
  container_access_type = "private"
}

# ---------------------------------------------------------------------------
# Lifecycle: cool after 30d, delete after 90d. Caller can shadow this with
# their own policy if compliance dictates longer retention.
# ---------------------------------------------------------------------------

resource "azurerm_storage_management_policy" "lifecycle" {
  storage_account_id = azurerm_storage_account.sa.id

  rule {
    name    = "bundles-tier-and-expire"
    enabled = true

    filters {
      blob_types   = ["blockBlob"]
      prefix_match = ["${var.naming.storage_container}/"]
    }

    actions {
      base_blob {
        tier_to_cool_after_days_since_modification_greater_than = 30
        delete_after_days_since_modification_greater_than       = 90
      }
      snapshot {
        delete_after_days_since_creation_greater_than = 30
      }
    }
  }
}

# ---------------------------------------------------------------------------
# Private endpoint
# ---------------------------------------------------------------------------

resource "azurerm_private_dns_zone" "blob" {
  name                = var.naming.storage_pdz
  resource_group_name = var.resource_group_name
  tags                = var.tags
}

resource "azurerm_private_dns_zone_virtual_network_link" "blob" {
  name                  = "${var.naming.storage}-pdzvl"
  resource_group_name   = var.resource_group_name
  private_dns_zone_name = azurerm_private_dns_zone.blob.name
  virtual_network_id    = var.vnet_id
  registration_enabled  = false
}

resource "azurerm_private_endpoint" "blob" {
  name                = "${var.naming.storage}-pe"
  location            = var.location
  resource_group_name = var.resource_group_name
  subnet_id           = var.pe_subnet_id
  tags                = var.tags

  private_service_connection {
    name                           = "${var.naming.storage}-psc"
    private_connection_resource_id = azurerm_storage_account.sa.id
    is_manual_connection           = false
    subresource_names              = ["blob"]
  }

  private_dns_zone_group {
    name                 = "blob-pdzg"
    private_dns_zone_ids = [azurerm_private_dns_zone.blob.id]
  }
}

# ---------------------------------------------------------------------------
# Diagnostic settings
# ---------------------------------------------------------------------------

resource "azurerm_monitor_diagnostic_setting" "sa" {
  name                       = "${var.naming.storage}-diag"
  target_resource_id         = "${azurerm_storage_account.sa.id}/blobServices/default"
  log_analytics_workspace_id = var.log_analytics_id

  enabled_log {
    category = "StorageRead"
  }
  enabled_log {
    category = "StorageWrite"
  }
  enabled_log {
    category = "StorageDelete"
  }

  enabled_metric {
    category = "Transaction"
  }
}
