region = "us-east-1"

availability_zones = ["us-east-1a", "us-east-1b", "us-east-1c"]

vpc_cidr = "10.0.0.0/16"

# EKS: small node group for dev
eks_node_instance_types = ["m6i.large"]
eks_desired_size        = 2
eks_min_size            = 2
eks_max_size            = 5
eks_disk_size           = 50

# RDS: single-AZ, minimal storage
rds_instance_class        = "db.r6g.large"
rds_allocated_storage     = 50
rds_multi_az              = false
rds_backup_retention_days = 7
rds_deletion_protection   = false

# Redis: single shard, no replicas
redis_node_type         = "cache.r6g.large"
redis_cluster_enabled   = false
redis_num_shards        = 1
redis_replicas_per_shard = 0
redis_automatic_failover = false
redis_backup_retention_days = 7

# Monitoring: minimal for dev
monitoring_enable_ingress      = false
monitoring_prometheus_retention = "7d"
monitoring_prometheus_storage   = "20Gi"
monitoring_grafana_storage      = "5Gi"
monitoring_grafana_admin_user   = "admin"
