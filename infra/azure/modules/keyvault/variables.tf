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

variable "tenant_id" {
  type = string
}

variable "kv_admin_object_id" {
  description = "Entra group/user that gets full KV admin (cert + secret manage)."
  type        = string
}

variable "allow_public_access" {
  description = "If true, leaves KV firewall in Allow mode. False locks it to private endpoint + AzureServices bypass."
  type        = bool
}

variable "pe_subnet_id" {
  type = string
}

variable "vnet_id" {
  type = string
}
