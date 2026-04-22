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
  description = "Pre-rendered resource names from the parent's locals.naming map."
  type        = map(string)
}

variable "vnet_address_space" {
  type = list(string)
}

variable "appgw_subnet_cidr" {
  type = string
}

variable "aca_subnet_cidr" {
  type = string
}

variable "postgres_subnet_cidr" {
  type = string
}

variable "pe_subnet_cidr" {
  type = string
}
