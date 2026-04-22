output "appgw_id" {
  value = azurerm_application_gateway.appgw.id
}

output "public_ip" {
  value = azurerm_public_ip.appgw.ip_address
}

output "public_dns_name" {
  description = "Azure-managed *.cloudapp.azure.com FQDN — useful for testing before customer DNS is cut over."
  value       = azurerm_public_ip.appgw.fqdn
}

output "waf_policy_id" {
  value = azurerm_web_application_firewall_policy.wafp.id
}
