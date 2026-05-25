variable "environment" {
  description = "Environment name (dev/prod)"
  type        = string
}

variable "project_name" {
  description = "Project name for resource naming"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID"
  type        = string
}

variable "database_subnet_ids" {
  description = "List of database subnet IDs"
  type        = list(string)
}

variable "eks_security_group_id" {
  description = "Security group ID for EKS nodes"
  type        = string
}

variable "node_type" {
  description = "ElastiCache node type"
  type        = string
  default     = "cache.r6g.large"
}

variable "cluster_enabled" {
  description = "Enable cluster mode"
  type        = bool
  default     = true
}

variable "num_shards" {
  description = "Number of shards in cluster mode"
  type        = number
  default     = 3
}

variable "replicas_per_shard" {
  description = "Number of replicas per shard"
  type        = number
  default     = 2
}

variable "automatic_failover" {
  description = "Enable automatic failover"
  type        = bool
  default     = true
}

variable "backup_retention_days" {
  description = "Backup retention period in days"
  type        = number
  default     = 7
}

variable "common_tags" {
  description = "Common tags for all resources"
  type        = map(string)
  default     = {}
}
