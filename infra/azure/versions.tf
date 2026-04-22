terraform {
  required_version = ">= 1.6.0"

  required_providers {
    # azurerm 4.x is the current stable line as of 2026-04. Pinned to ~> 4.0
    # so callers on 4.x stay on minor-version drift inside the major.
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 4.0"
    }

    # azapi covers the surface azurerm doesn't yet expose — primarily ACA
    # workload profiles, ACA secrets-from-keyvault references with the new
    # identity binding, and a few AppGW v2 mTLS knobs (clientAuthConfiguration
    # on SSL profiles, trustedClientCertificates lifecycle).
    azapi = {
      source  = "Azure/azapi"
      version = "~> 2.0"
    }

    random = {
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
  }
}
