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

variable "pe_subnet_id" {
  type = string
}

variable "vnet_id" {
  type = string
}

variable "log_analytics_id" {
  type = string
}
