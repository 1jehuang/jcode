# ─────────────────────────────────────────────────────
# CarpAI - Dev Environment
# ─────────────────────────────────────────────────────

locals {
  common_tags = {
    Project     = var.project_name
    Environment = var.environment
    ManagedBy   = "terraform"
    Owner       = var.owner
  }
}

module "carpai" {
  source = "../../"

  region               = var.region
  availability_zones   = var.availability_zones
  project_name         = var.project_name
  environment          = var.environment
  common_tags          = local.common_tags
  owner                = var.owner

  # EKS
  vpc_cidr              = var.vpc_cidr
  eks_cluster_version   = "1.28"
  eks_node_instance_types = var.eks_node_instance_types
  eks_desired_size      = var.eks_desired_size
  eks_min_size          = var.eks_min_size
  eks_max_size          = var.eks_max_size
  eks_disk_size         = var.eks_disk_size

  # RDS
  rds_instance_class        = var.rds_instance_class
  rds_allocated_storage     = var.rds_allocated_storage
  rds_max_allocated_storage = 100
  rds_multi_az              = var.rds_multi_az
  rds_backup_retention_days = var.rds_backup_retention_days
  rds_deletion_protection   = var.rds_deletion_protection
  rds_postgres_version      = "15"

  # Redis
  redis_node_type         = var.redis_node_type
  redis_cluster_enabled   = var.redis_cluster_enabled
  redis_num_shards        = var.redis_num_shards
  redis_replicas_per_shard = var.redis_replicas_per_shard
  redis_automatic_failover = var.redis_automatic_failover
  redis_backup_retention_days = var.redis_backup_retention_days

  # Monitoring
  monitoring_enable_ingress      = var.monitoring_enable_ingress
  monitoring_prometheus_retention = var.monitoring_prometheus_retention
  monitoring_prometheus_storage   = var.monitoring_prometheus_storage
  monitoring_grafana_storage      = var.monitoring_grafana_storage
  monitoring_grafana_admin_user   = var.monitoring_grafana_admin_user
}
