# ─────────────────────────────────────────────────────
# Security Group
# ─────────────────────────────────────────────────────
resource "aws_security_group" "redis" {
  name        = "${var.project_name}-${var.environment}-redis"
  description = "Security group for ElastiCache Redis"
  vpc_id      = var.vpc_id

  ingress {
    description     = "Redis from EKS nodes"
    from_port       = 6379
    to_port         = 6379
    protocol        = "tcp"
    security_groups = [var.eks_security_group_id]
  }

  tags = merge(var.common_tags, {
    Name = "${var.project_name}-${var.environment}-redis-sg"
  })
}

# ─────────────────────────────────────────────────────
# Parameter Group
# ─────────────────────────────────────────────────────
resource "aws_elasticache_parameter_group" "main" {
  name        = "${var.project_name}-${var.environment}-redis7"
  family      = "redis7"
  description = "Redis 7 parameter group for CarpAI"

  dynamic "parameter" {
    for_each = var.cluster_enabled ? [1] : []
    content {
      name  = "cluster-enabled"
      value = "yes"
    }
  }

  tags = var.common_tags
}

# ─────────────────────────────────────────────────────
# Subnet Group
# ─────────────────────────────────────────────────────
resource "aws_elasticache_subnet_group" "main" {
  name       = "${var.project_name}-${var.environment}-redis-subnet"
  subnet_ids = var.database_subnet_ids

  tags = var.common_tags
}

# ─────────────────────────────────────────────────────
# Redis Replication Group
# ─────────────────────────────────────────────────────
resource "aws_elasticache_replication_group" "main" {
  replication_group_id          = "${var.project_name}-${var.environment}"
  description                   = "CarpAI Redis cluster - ${var.environment}"
  node_type                     = var.node_type
  port                          = 6379
  parameter_group_name          = aws_elasticache_parameter_group.main.name
  subnet_group_name             = aws_elasticache_subnet_group.main.name
  security_group_ids            = [aws_security_group.redis.id]
  engine                        = "redis"
  engine_version                = "7.0"

  automatic_failover_enabled    = var.automatic_failover
  multi_az_enabled              = var.automatic_failover

  # Cluster mode
  cluster_mode {
    replicas_per_node_group = var.replicas_per_shard
    num_node_groups         = var.num_shards
  }

  # Backup
  snapshot_retention_limit     = var.backup_retention_days
  snapshot_window              = "03:00-04:00"
  auto_minor_version_upgrade   = true
  maintenance_window           = "sun:06:00-sun:07:00"

  at_rest_encryption_enabled   = true
  transit_encryption_enabled   = true

  tags = var.common_tags
}
