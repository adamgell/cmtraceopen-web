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

variable "postgres_subnet_id" {
  type = string
}

variable "vnet_id" {
  type = string
}

variable "sku_name" {
  type = string
}

variable "storage_mb" {
  type = number
}

variable "admin_login" {
  type = string
}

variable "aad_admin_object_id" {
  description = "Entra group/user that gets Postgres AAD admin. Should be a group so on-call can rotate."
  type        = string
}

variable "aad_admin_tenant_id" {
  type = string
}

variable "log_analytics_id" {
  type = string
}

variable "key_vault_id" {
  description = "Where the generated admin password lands as a secret. The containerapp module reads from here."
  type        = string
}
