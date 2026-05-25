# ──────────────────────────────────────────────
# Provider Configuration
# ──────────────────────────────────────────────

provider "aws" {
  region = var.region
  default_tags {
    tags = merge(var.common_tags, {
      Environment = var.environment
      Project     = var.project_name
      ManagedBy   = "terraform"
      Owner       = var.owner
    })
  }
}

provider "kubernetes" {
  host                   = module.eks.cluster_endpoint
  cluster_ca_certificate = base64decode(module.eks.cluster_ca_certificate)
  token                  = module.eks.cluster_token
}

provider "helm" {
  kubernetes {
    host                   = module.eks.cluster_endpoint
    cluster_ca_certificate = base64decode(module.eks.cluster_ca_certificate)
    token                  = module.eks.cluster_token
  }
}

# ──────────────────────────────────────────────
# State Backend (S3 - commented out, local by default)
# ──────────────────────────────────────────────
# Uncomment and configure for remote state management:
# terraform {
#   backend "s3" {
#     bucket         = "carpai-terraform-state"
#     key            = "carpai/${var.environment}/terraform.tfstate"
#     region         = "us-east-1"
#     encrypt        = true
#     dynamodb_table = "carpai-terraform-locks"
#   }
# }

# ──────────────────────────────────────────────
# Data Sources
# ──────────────────────────────────────────────

data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

locals {
  name_prefix = "${var.project_name}-${var.environment}"
}

# ──────────────────────────────────────────────
# Networking Module
# ──────────────────────────────────────────────

module "networking" {
  source = "./modules/networking"

  environment         = var.environment
  project_name        = var.project_name
  vpc_cidr            = var.vpc_cidr
  availability_zones  = var.availability_zones
  common_tags         = var.common_tags
}

# ──────────────────────────────────────────────
# EKS Module
# ──────────────────────────────────────────────

module "eks" {
  source = "./modules/eks"

  environment        = var.environment
  project_name       = var.project_name
  vpc_id             = module.networking.vpc_id
  subnet_ids         = module.networking.private_subnet_ids
  cluster_version    = var.eks_cluster_version
  instance_types     = var.eks_node_instance_types
  min_size           = var.eks_min_size
  max_size           = var.eks_max_size
  desired_size       = var.eks_desired_size
  disk_size          = var.eks_disk_size
  common_tags        = var.common_tags

  depends_on = [module.networking]
}

# ──────────────────────────────────────────────
# RDS Module
# ──────────────────────────────────────────────

module "rds" {
  source = "./modules/rds"

  environment           = var.environment
  project_name          = var.project_name
  vpc_id                = module.networking.vpc_id
  database_subnet_ids   = module.networking.database_subnet_ids
  eks_security_group_id = module.networking.eks_sg_id
  instance_class        = var.rds_instance_class
  allocated_storage     = var.rds_allocated_storage
  max_allocated_storage = var.rds_max_allocated_storage
  multi_az              = var.rds_multi_az
  backup_retention_days = var.rds_backup_retention_days
  deletion_protection   = var.rds_deletion_protection
  postgres_version      = var.rds_postgres_version
  common_tags           = var.common_tags

  depends_on = [module.networking]
}

# ──────────────────────────────────────────────
# Redis Module
# ──────────────────────────────────────────────

module "redis" {
  source = "./modules/redis"

  environment           = var.environment
  project_name          = var.project_name
  vpc_id                = module.networking.vpc_id
  database_subnet_ids   = module.networking.database_subnet_ids
  eks_security_group_id = module.networking.eks_sg_id
  node_type             = var.redis_node_type
  cluster_enabled       = var.redis_cluster_enabled
  num_shards            = var.redis_num_shards
  replicas_per_shard    = var.redis_replicas_per_shard
  automatic_failover    = var.redis_automatic_failover
  backup_retention_days = var.redis_backup_retention_days
  common_tags           = var.common_tags

  depends_on = [module.networking]
}

# ──────────────────────────────────────────────
# Monitoring Module (Prometheus + Grafana)
# ──────────────────────────────────────────────

module "monitoring" {
  source = "./modules/monitoring"

  environment           = var.environment
  project_name          = var.project_name
  prometheus_retention  = var.monitoring_prometheus_retention
  prometheus_storage    = var.monitoring_prometheus_storage
  grafana_storage       = var.monitoring_grafana_storage
  grafana_admin_user    = var.monitoring_grafana_admin_user
  enable_ingress        = var.monitoring_enable_ingress
  common_tags           = var.common_tags

  depends_on = [module.eks]
}

# ──────────────────────────────────────────────
# CarpAI Helm Chart Deployment
# ──────────────────────────────────────────────

resource "helm_release" "carpai" {
  name       = "carpai"
  namespace  = "carpai-system"
  create_namespace = true

  chart      = "${path.module}/../helm/carpai"
  depends_on = [module.eks, module.rds, module.redis, module.monitoring]

  set {
    name  = "image.tag"
    value = "latest"
  }

  set {
    name  = "environment"
    value = var.environment
  }

  set {
    name  = "config.database.host"
    value = module.rds.endpoint
  }

  set {
    name  = "config.database.port"
    value = module.rds.port
  }

  set {
    name  = "config.database.name"
    value = module.rds.database_name
  }

  set {
    name  = "config.database.username"
    value = module.rds.master_username
  }

  set {
    name  = "config.database.passwordSecret"
    value = module.rds.master_password_secret_arn
  }

  set {
    name  = "config.redis.host"
    value = module.redis.primary_endpoint
  }

  set {
    name  = "config.redis.port"
    value = module.redis.port
  }

  set {
    name  = "monitoring.prometheus.url"
    value = module.monitoring.prometheus_url
  }

  set {
    name  = "monitoring.grafana.url"
    value = module.monitoring.grafana_url
  }

  set {
    name  = "ingress.enabled"
    value = var.monitoring_enable_ingress
  }
}
