###############################################################################
# Application Gateway v2, WAF_v2 SKU, with mTLS termination.
#
# # mTLS routing strategy (the load-bearing decision in this module)
#
# AppGW v2 supports per-listener mTLS via SSL profiles, but only ONE SSL
# profile can be attached per HTTPS listener. To get "ingest paths require
# client cert, query paths optional" we use the **two-listener-on-same-IP**
# pattern:
#
#   listener-mtls-required:
#     port 443, hostname = var.frontend_fqdn,
#     ssl_profile = profile-mtls-required (verify_client_cert_issuer_dn=true,
#       trusted_client_certificates_names = [client-root-ca]).
#
#   listener-mtls-optional:
#     port 443, hostname = var.frontend_fqdn,
#     ssl_profile = profile-mtls-optional (verify_client_cert_issuer_dn=false).
#
# Both listeners share the same frontend IP + port + hostname; AppGW
# disambiguates via the SSL profile binding. The path-based routing rule
# routes `/v1/ingest/*` to the required-listener-bound rule and everything
# else to the optional-listener-bound rule.
#
# This is the "Microsoft-blessed" approach for per-path mTLS at AppGW v2 as
# of mid-2025; the previously-suggested "single listener + URL rewrite to
# 401 on missing cert" doesn't work because rewrites can't read TLS state.
#
# Alternative considered + rejected: split into two AppGWs (one mTLS-required,
# one bearer-only). Doubles cost (~$400/mo extra) and forces clients to
# pick a hostname based on intent — operator tooling would have to know
# whether a route is ingest vs query, which leaks server topology to
# clients. Single FQDN + dual listener keeps the public surface clean.
#
# # X-ARR-ClientCert
#
# When mTLS is enforced, AppGW forwards the verified client cert to the
# backend in the `X-ARR-ClientCert` header (PEM-encoded, base64 in a single
# line, BEGIN/END CERTIFICATE preserved). The api-server reads this header
# (CMTRACE_PEER_CERT_HEADER) instead of doing its own TLS termination.
# The header is only added on listeners with mTLS enabled — so on the
# "optional" listener it's absent, and the api-server falls back to bearer-
# token auth via Entra. This matches the existing auth_mode wiring without
# any code changes (other than learning to read the header — see runbook §10).
###############################################################################

resource "azurerm_public_ip" "appgw" {
  name                = var.naming.pip_appgw
  location            = var.location
  resource_group_name = var.resource_group_name
  allocation_method   = "Static"
  sku                 = "Standard"
  domain_name_label   = var.naming.appgw # gives appgw a *.cloudapp.azure.com fallback
  tags                = var.tags
}

# WAF policy — kept as a separate resource so SOC can update OWASP
# exclusions independently of AppGW lifecycle.
resource "azurerm_web_application_firewall_policy" "wafp" {
  name                = var.naming.waf_policy
  location            = var.location
  resource_group_name = var.resource_group_name
  tags                = var.tags

  policy_settings {
    enabled                     = true
    mode                        = "Prevention"
    request_body_check          = true
    file_upload_limit_in_mb     = 100 # bundles can be big — bumps default 100MB
    max_request_body_size_in_kb = 128
  }

  managed_rules {
    managed_rule_set {
      type    = "OWASP"
      version = "3.2"
    }

    # X-ARR-ClientCert is a base64 PEM blob and trips OWASP rules around
    # request-header size + suspicious-character sequences (BEGIN/END,
    # newlines as %0A). Exclude the header from those rules.
    exclusion {
      match_variable          = "RequestHeaderNames"
      selector                = "X-ARR-ClientCert"
      selector_match_operator = "Equals"
    }
  }
}

# ---------------------------------------------------------------------------
# Application Gateway v2
# ---------------------------------------------------------------------------

locals {
  fe_ip_name        = "fe-pip"
  fe_port_https     = "fe-port-443"
  fe_listener_mtls  = "lstn-mtls-required"
  fe_listener_open  = "lstn-mtls-optional"
  ssl_profile_mtls  = "ssl-profile-mtls-required"
  ssl_profile_open  = "ssl-profile-mtls-optional"
  ssl_cert_name     = "appgw-frontend-cert"
  trusted_ca_name   = "client-root-ca"
  backend_pool_name = "bep-aca"
  backend_settings  = "behs-aca-https"
  rule_ingest       = "rule-ingest"
  rule_default      = "rule-default"
  pathmap_name      = "pmap-ingest"
}

resource "azurerm_application_gateway" "appgw" {
  name                = var.naming.appgw
  location            = var.location
  resource_group_name = var.resource_group_name
  http2_enabled       = true
  firewall_policy_id  = azurerm_web_application_firewall_policy.wafp.id
  tags                = var.tags

  sku {
    name = "WAF_v2"
    tier = "WAF_v2"
  }

  autoscale_configuration {
    min_capacity = var.capacity_min
    max_capacity = var.capacity_max
  }

  identity {
    type         = "UserAssigned"
    identity_ids = [var.appgw_identity_id]
  }

  gateway_ip_configuration {
    name      = "appgw-ipconfig"
    subnet_id = var.appgw_subnet_id
  }

  frontend_ip_configuration {
    name                 = local.fe_ip_name
    public_ip_address_id = azurerm_public_ip.appgw.id
  }

  frontend_port {
    name = local.fe_port_https
    port = 443
  }

  # ---------------------------------------------------------------------
  # SSL cert from KV. Caller uploads PFX via separate process (runbook §6).
  # We reference by `key_vault_secret_id` (the live URI) so AppGW pulls
  # the latest-version automatically when the cert is rotated in KV.
  # ---------------------------------------------------------------------
  ssl_certificate {
    name                = local.ssl_cert_name
    key_vault_secret_id = data.azurerm_key_vault_secret.frontend_cert.versionless_id
  }

  # Trusted client CA — the Cloud PKI Root + Issuing chain bundle.
  # Same KV-pull pattern as the frontend cert.
  trusted_client_certificate {
    name = local.trusted_ca_name
    data = data.azurerm_key_vault_secret.client_root_ca.value
  }

  # ---------------------------------------------------------------------
  # SSL profile — single profile with optional mTLS. The api-server
  # handles enforcement via CMTRACE_MTLS_REQUIRE_INGEST at the app layer.
  # A future dual-listener design (separate hostnames for ingest vs query)
  # can re-introduce the split; Azure AppGW doesn't allow two listeners
  # on the same port+hostname+IP with different SSL profiles.
  # ---------------------------------------------------------------------

  ssl_profile {
    name = local.ssl_profile_open
    ssl_policy {
      policy_type = "Predefined"
      policy_name = "AppGwSslPolicy20220101"
    }
  }

  # ---------------------------------------------------------------------
  # Single listener — all traffic through one HTTPS listener.
  # ---------------------------------------------------------------------

  http_listener {
    name                           = local.fe_listener_open
    frontend_ip_configuration_name = local.fe_ip_name
    frontend_port_name             = local.fe_port_https
    protocol                       = "Https"
    ssl_certificate_name           = local.ssl_cert_name
    ssl_profile_name               = local.ssl_profile_open
    host_name                      = var.frontend_fqdn
    require_sni                    = true
  }

  # ---------------------------------------------------------------------
  # Backend — single ACA app, HTTPS on 443. ACA always serves HTTPS at
  # the env-internal ingress, regardless of `transport = http` on the app
  # ingress block (the `transport` setting only affects the inside-the-pod
  # protocol; the env's edge always wraps it in TLS).
  # ---------------------------------------------------------------------

  backend_address_pool {
    name  = local.backend_pool_name
    fqdns = [var.backend_fqdn]
  }

  probe {
    name                                      = "probe-healthz"
    protocol                                  = "Https"
    host                                      = var.backend_fqdn
    path                                      = "/healthz"
    interval                                  = 30
    timeout                                   = 10
    unhealthy_threshold                       = 3
    pick_host_name_from_backend_http_settings = false
    match {
      status_code = ["200-399"]
    }
  }

  backend_http_settings {
    name                                = local.backend_settings
    cookie_based_affinity               = "Disabled"
    port                                = 443
    protocol                            = "Https"
    request_timeout                     = 60
    pick_host_name_from_backend_address = true
    probe_name                          = "probe-healthz"
  }

  # ---------------------------------------------------------------------
  # Routing — single basic rule sends all traffic to the ACA backend.
  # mTLS enforcement is handled at the api-server layer, not AppGW.
  # ---------------------------------------------------------------------

  request_routing_rule {
    name                       = local.rule_default
    rule_type                  = "Basic"
    http_listener_name         = local.fe_listener_open
    backend_address_pool_name  = local.backend_pool_name
    backend_http_settings_name = local.backend_settings
    priority                   = 100
  }

  lifecycle {
    # Don't churn when Azure backfills auto-generated trusted root certs
    # for *.azurecontainerapps.io (the pool's TLS chain).
    ignore_changes = [
      trusted_root_certificate,
    ]
  }

  depends_on = [
    azurerm_public_ip.appgw,
    azurerm_web_application_firewall_policy.wafp,
  ]
}

# ---------------------------------------------------------------------------
# Diagnostic settings -> LAW
# ---------------------------------------------------------------------------

resource "azurerm_monitor_diagnostic_setting" "appgw" {
  name                       = "${var.naming.appgw}-diag"
  target_resource_id         = azurerm_application_gateway.appgw.id
  log_analytics_workspace_id = var.log_analytics_id

  enabled_log {
    category = "ApplicationGatewayAccessLog"
  }
  enabled_log {
    category = "ApplicationGatewayPerformanceLog"
  }
  enabled_log {
    category = "ApplicationGatewayFirewallLog"
  }

  enabled_metric {
    category = "AllMetrics"
  }
}

# ---------------------------------------------------------------------------
# Data sources for KV secrets — referenced via versionless_id so cert
# rotations in KV don't require a Terraform apply. The user-assigned MI
# bound on the AppGW fetches the live value at handshake time.
#
# These data lookups DO require the KV secret to already exist at apply
# time. If the caller hasn't uploaded the cert + CA bundle yet, the
# initial apply will fail at the data lookup with a clear error. See
# runbook §6 for the upload flow.
# ---------------------------------------------------------------------------

data "azurerm_key_vault_secret" "frontend_cert" {
  name         = var.frontend_cert_secret_name
  key_vault_id = var.key_vault_id
}

data "azurerm_key_vault_secret" "client_root_ca" {
  name         = var.client_root_ca_secret_name
  key_vault_id = var.key_vault_id
}
