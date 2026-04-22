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

variable "aca_subnet_id" {
  type = string
}

variable "log_analytics_id" {
  type = string
}

variable "log_analytics_key_id" {
  description = "LAW workspace ID (the GUID, not the resource ID). ACA env diagnostic config wants both."
  type        = string
}

variable "image" {
  type = string
}

variable "image_pull_secret_name" {
  description = "KV secret name holding registry pull PAT. Empty = public image."
  type        = string
}

variable "use_workload_profile" {
  type = bool
}

variable "min_replicas" {
  type = number
}

variable "max_replicas" {
  type = number
}

variable "cpu" {
  type = number
}

variable "memory" {
  type = string
}

variable "static_env_vars" {
  description = "Map of env-var name -> literal value. Wired into the container app via for_each."
  type        = map(string)
}

variable "key_vault_id" {
  type = string
}

variable "key_vault_uri" {
  type = string
}

variable "postgres_url_secret_id" {
  description = "Resource ID of the KV secret holding CMTRACE_DATABASE_URL."
  type        = string
}

variable "storage_account_name" {
  type = string
}

variable "storage_account_id" {
  type = string
}

variable "postgres_server_fqdn" {
  type = string
}

variable "postgres_database_name" {
  type = string
}
