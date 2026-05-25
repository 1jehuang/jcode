variable "region" {
  description = "AWS region to deploy resources"
  type        = string
  default     = "us-east-1"
}

variable "environment" {
  description = "Deployment environment (dev, staging, prod)"
  type        = string
}

variable "project_name" {
  description = "Project name for resource naming and tagging"
  type        = string
  default     = "carpai"
}

variable "owner" {
  description = "Team or person responsible for these resources"
  type        = string
  default     = "platform-team"
}

variable "vpc_cidr" {
  description = "CIDR block for the VPC"
  type        = string
  default     = "10.0.0.0/16"
}

variable "availability_zones" {
  description = "List of availability zones to use"
  type        = list(string)
  default     = ["us-east-1a", "us-east-1b", "us-east-1c"]
}

variable "eks_cluster_version" {
  description = "Kubernetes version for EKS cluster"
  type        = string
  default     = "1.28"
}

variable "eks_node_instance_types" {
  description = "EC2 instance types for EKS managed node group"
  type        = list(string)
  default     = ["m6i.large"]
}

variable "eks_min_size" {
  description = "Minimum number of nodes in EKS node group"
  type        = number
  default     = 2
}

variable "eks_max_size" {
  description = "Maximum number of nodes in EKS node group"
  type        = number
  default     = 20
}

variable "eks_desired_size" {
  description = "Desired number of nodes in EKS node group"
  type        = number
  default     = 3
}

variable "eks_disk_size" {
  description = "Disk size in GB for EKS nodes"
  type        = number
  default     = 80
}

variable "rds_instance_class" {
  description = "RDS instance class"
  type        = string
  default     = "db.r6g.large"
}

variable "rds_allocated_storage" {
  description = "Allocated storage for RDS in GB"
  type        = number
  default     = 50
}

variable "rds_max_allocated_storage" {
  description = "Maximum storage for RDS autoscaling in GB"
  type        = number
  default     = 200
}

variable "rds_multi_az" {
  description = "Enable Multi-AZ for RDS"
  type        = bool
  default     = false
}

variable "rds_backup_retention_days" {
  description = "Number of days to retain RDS backups"
  type        = number
  default     = 7
}

variable "rds_deletion_protection" {
  description = "Enable deletion protection for RDS"
  type        = bool
  default     = false
}

variable "rds_postgres_version" {
  description = "PostgreSQL engine version"
  type        = string
  default     = "15"
}

variable "redis_node_type" {
  description = "ElastiCache Redis node type"
  type        = string
  default     = "cache.r6g.large"
}

variable "redis_cluster_enabled" {
  description = "Enable Redis cluster mode"
  type        = bool
  default     = false
}

variable "redis_num_shards" {
  description = "Number of Redis shards (cluster mode only)"
  type        = number
  default     = 1
}

variable "redis_replicas_per_shard" {
  description = "Number of replicas per Redis shard"
  type        = number
  default     = 0
}

variable "redis_automatic_failover" {
  description = "Enable automatic failover for Redis"
  type        = bool
  default     = false
}

variable "redis_backup_retention_days" {
  description = "Number of days to retain Redis backups"
  type        = number
  default     = 7
}

variable "monitoring_prometheus_retention" {
  description = "Prometheus data retention period"
  type        = string
  default     = "15d"
}

variable "monitoring_prometheus_storage" {
  description = "Prometheus persistent storage size"
  type        = string
  default     = "50Gi"
}

variable "monitoring_grafana_storage" {
  description = "Grafana persistent storage size"
  type        = string
  default     = "10Gi"
}

variable "monitoring_grafana_admin_user" {
  description = "Grafana admin username"
  type        = string
  default     = "admin"
}

variable "monitoring_enable_ingress" {
  description = "Enable ingress for Grafana and Prometheus"
  type        = bool
  default     = false
}

variable "common_tags" {
  description = "Common tags applied to all resources"
  type        = map(string)
  default     = {}
}
