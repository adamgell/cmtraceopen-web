###############################################################################
# Single VNet topology with four subnets.
#
# We keep the "hub/spoke" framing in the design doc but materialize it as a
# single VNet here for simplicity — splitting hub/spoke costs an extra
# peering pair and a second NSG set per environment, neither of which buys
# anything until the caller is co-locating multiple workloads. Promote to
# real hub/spoke by:
#   1. Pulling the AppGW + KV PE subnets out into a hub VNet variable, and
#   2. Adding `azurerm_virtual_network_peering` blocks. Both changes are
#      additive — none of the resource IDs this submodule outputs change.
#
# Subnet purposes:
#   appgw_subnet     — Application Gateway v2; needs /24 minimum to autoscale.
#   aca_subnet       — Container Apps environment (workload-profile or
#                      consumption); requires /23 for workload-profile,
#                      /27 for consumption-only. We default /23 to leave
#                      room for either.
#   postgres_subnet  — Delegated to Microsoft.DBforPostgreSQL/flexibleServers.
#   pe_subnet        — Private endpoints for Key Vault + Storage. Separate
#                      from ACA so PE NSG rules stay narrow.
###############################################################################

resource "azurerm_virtual_network" "vnet" {
  name                = var.naming.vnet
  location            = var.location
  resource_group_name = var.resource_group_name
  address_space       = var.vnet_address_space
  tags                = var.tags
}

resource "azurerm_subnet" "appgw" {
  name                 = "snet-appgw"
  resource_group_name  = var.resource_group_name
  virtual_network_name = azurerm_virtual_network.vnet.name
  address_prefixes     = [var.appgw_subnet_cidr]
}

resource "azurerm_subnet" "aca" {
  name                 = "snet-aca"
  resource_group_name  = var.resource_group_name
  virtual_network_name = azurerm_virtual_network.vnet.name
  address_prefixes     = [var.aca_subnet_cidr]

  # ACA workload-profile envs require this delegation; consumption-only
  # also accepts it. Setting it unconditionally is safe and avoids a
  # subnet recreate when the caller flips var.aca_use_workload_profile.
  delegation {
    name = "aca-delegation"
    service_delegation {
      name    = "Microsoft.App/environments"
      actions = ["Microsoft.Network/virtualNetworks/subnets/join/action"]
    }
  }
}

resource "azurerm_subnet" "postgres" {
  name                 = "snet-postgres"
  resource_group_name  = var.resource_group_name
  virtual_network_name = azurerm_virtual_network.vnet.name
  address_prefixes     = [var.postgres_subnet_cidr]

  delegation {
    name = "pg-delegation"
    service_delegation {
      name    = "Microsoft.DBforPostgreSQL/flexibleServers"
      actions = ["Microsoft.Network/virtualNetworks/subnets/join/action"]
    }
  }

  service_endpoints = ["Microsoft.Storage"]
}

resource "azurerm_subnet" "pe" {
  name                              = "snet-pe"
  resource_group_name               = var.resource_group_name
  virtual_network_name              = azurerm_virtual_network.vnet.name
  address_prefixes                  = [var.pe_subnet_cidr]
  private_endpoint_network_policies = "Disabled"
}

# ---------------------------------------------------------------------------
# NSGs — one per subnet so blast radius of a rule edit is local.
# ---------------------------------------------------------------------------

resource "azurerm_network_security_group" "appgw" {
  name                = var.naming.nsg_appgw
  location            = var.location
  resource_group_name = var.resource_group_name
  tags                = var.tags

  # AppGW v2 mandatory inbound rules (per Azure docs).
  security_rule {
    name                       = "AllowGatewayManager"
    priority                   = 100
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "Tcp"
    source_port_range          = "*"
    destination_port_range     = "65200-65535"
    source_address_prefix      = "GatewayManager"
    destination_address_prefix = "*"
  }
  security_rule {
    name                       = "AllowAzureLoadBalancer"
    priority                   = 110
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "*"
    source_port_range          = "*"
    destination_port_range     = "*"
    source_address_prefix      = "AzureLoadBalancer"
    destination_address_prefix = "*"
  }
  security_rule {
    name                       = "AllowHTTPSInbound"
    priority                   = 120
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "Tcp"
    source_port_range          = "*"
    destination_port_range     = "443"
    source_address_prefix      = "Internet"
    destination_address_prefix = "*"
  }
  # Explicit deny-all is added by Azure; no extra rule needed.
}

resource "azurerm_network_security_group" "aca" {
  name                = var.naming.nsg_aca
  location            = var.location
  resource_group_name = var.resource_group_name
  tags                = var.tags

  # ACA env requires VirtualNetwork-tag-based egress to AzureLoadBalancer
  # for the control plane. Inbound is locked to the AppGW subnet only.
  security_rule {
    name                       = "AllowAppGwInbound"
    priority                   = 100
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "Tcp"
    source_port_range          = "*"
    destination_port_range     = "443"
    source_address_prefix      = var.appgw_subnet_cidr
    destination_address_prefix = "*"
  }
}

resource "azurerm_network_security_group" "pe" {
  name                = var.naming.nsg_pe
  location            = var.location
  resource_group_name = var.resource_group_name
  tags                = var.tags
  # Default deny-inbound is fine; PE NICs are reached only from inside the VNet.
}

resource "azurerm_subnet_network_security_group_association" "appgw" {
  subnet_id                 = azurerm_subnet.appgw.id
  network_security_group_id = azurerm_network_security_group.appgw.id
}

resource "azurerm_subnet_network_security_group_association" "aca" {
  subnet_id                 = azurerm_subnet.aca.id
  network_security_group_id = azurerm_network_security_group.aca.id
}

resource "azurerm_subnet_network_security_group_association" "pe" {
  subnet_id                 = azurerm_subnet.pe.id
  network_security_group_id = azurerm_network_security_group.pe.id
}
