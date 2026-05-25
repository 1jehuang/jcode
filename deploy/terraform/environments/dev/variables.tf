variable "region" {
  description = "AWS region for resources"
  type        = string
  default     = "us-east-1"
}

variable "availability_zones" {
  description = "List of availability zones"
  type        = list(string)
  default     = ["us-east-1a", "us-east-1b", "us-east-1c"]
}

variable "vpc_cidr" {
  description = "VPC CIDR block"
  type        = string
  default     = "10.0.0.0/16"
}

variable "eks_node_instance_types" {
  description = "EC2 instance types for EKS node group"
  type        = list(string)
  default     = ["m6i.large"]
}

variable "eks_desired_size" {
  description = "Desired number of EKS nodes"
  type        = number
  default     = 2
}

variable "eks_min_size" {
  description = "Minimum number of EKS nodes"
  type        = number
  default     = 2
}

variable "eks_max_size" {
  description = "Maximum number of EKS nodes"
  type        = number
  default     = 5
}

variable "eks_disk_size" {
  description = "EBS disk size (GB) for EKS nodes"
  type        = number
  default     = 50
}

variable "rds_instance_class" {
  description = "RDS instance class"
  type        = string
  default     = "db.r6g.large"
}

variable "rds_allocated_storage" {
  description = "Allocated storage for RDS"
  type        = number
  default     = 50
}

variable "rds_multi_az" {
  description = "Enable Multi-AZ for RDS"
  type        = bool
  default     = false
}

variable "rds_backup_retention_days" {
  description = "Backup retention for RDS"
  type        = number
  default     = 7
}

variable "rds_deletion_protection" {
  description = "Enable deletion protection for RDS"
  type        = bool
  default     = false
}

variable "redis_node_type" {
  description = "ElastiCache node type"
  type        = string
  default     = "cache.r6g.large"
}

variable "redis_cluster_enabled" {
  description = "Enable cluster mode for Redis"
  type        = bool
  default     = false
}

variable "redis_num_shards" {
  description = "Number of shards"
  type        = number
  default     = 1
}

variable "redis_replicas_per_shard" {
  description = "Number of replicas per shard"
  type        = number
  default     = 0
}

variable "redis_automatic_failover" {
  description = "Enable automatic failover"
  type        = bool
  default     = false
}

variable "redis_backup_retention_days" {
  description = "Backup retention for Redis"
  type        = number
  default     = 7
}

variable "monitoring_enable_ingress" {
  description = "Enable ingress for Grafana"
  type        = bool
  default     = false
}

variable "monitoring_prometheus_retention" {
  description = "Prometheus data retention"
  type        = string
  default     = "7d"
}

variable "monitoring_prometheus_storage" {
  description = "Prometheus storage size"
  type        = string
  default     = "20Gi"
}

variable "monitoring_grafana_storage" {
  description = "Grafana storage size"
  type        = string
  default     = "5Gi"
}

variable "monitoring_grafana_admin_user" {
  description = "Grafana admin username"
  type        = string
  default     = "admin"
}

variable "owner" {
  description = "Owner tag"
  type        = string
  default     = "dev-team"
}

variable "project_name" {
  description = "Project name"
  type        = string
  default     = "carpai"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "dev"
}
