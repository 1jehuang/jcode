variable "environment" {
  description = "Environment name (dev/prod)"
  type        = string
}

variable "project_name" {
  description = "Project name for resource naming"
  type        = string
}

variable "prometheus_retention" {
  description = "Prometheus data retention period"
  type        = string
  default     = "15d"
}

variable "prometheus_storage" {
  description = "Prometheus persistent storage size"
  type        = string
  default     = "50Gi"
}

variable "grafana_storage" {
  description = "Grafana persistent storage size"
  type        = string
  default     = "10Gi"
}

variable "grafana_admin_user" {
  description = "Grafana admin username"
  type        = string
  default     = "admin"
}

variable "grafana_admin_password" {
  description = "Grafana admin password (auto-generated if empty)"
  type        = string
  default     = ""
  sensitive   = true
}

variable "enable_ingress" {
  description = "Enable ingress for Grafana"
  type        = bool
  default     = false
}

variable "common_tags" {
  description = "Common tags for all resources"
  type        = map(string)
  default     = {}
}
