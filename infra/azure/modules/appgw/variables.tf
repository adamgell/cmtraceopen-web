variable "resource_group_name" {
  type = string
}

variable "location" {
  type = string
}

variable "tags" {
  type = map(string)
}

variable "naming" {
  type = map(string)
}

variable "appgw_subnet_id" {
  type = string
}

variable "log_analytics_id" {
  type = string
}

variable "capacity_min" {
  type = number
}

variable "capacity_max" {
  type = number
}

variable "frontend_fqdn" {
  type = string
}

variable "backend_fqdn" {
  description = "ACA app's internal ingress FQDN (e.g. cmtrace-pilot-cus-api.<env-id>.<region>.azurecontainerapps.io)."
  type        = string
}

variable "key_vault_id" {
  type = string
}

variable "frontend_cert_secret_name" {
  description = "KV secret name (NOT id) of the AppGW frontend cert. Caller uploads as a base64-encoded PFX before applying."
  type        = string
}

variable "client_root_ca_secret_name" {
  description = "KV secret name of the trusted client CA bundle (PEM, Cloud PKI Root + Issuing concatenated)."
  type        = string
}

variable "appgw_identity_principal_id" {
  type = string
}

variable "appgw_identity_id" {
  description = "Resource ID of the user-assigned MI the AppGW uses to pull KV secrets."
  type        = string
}
