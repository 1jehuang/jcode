region = "us-east-1"

availability_zones = ["us-east-1a", "us-east-1b", "us-east-1c"]

vpc_cidr = "10.0.0.0/16"

# EKS: larger nodes for production workload
eks_node_instance_types = ["m6i.xlarge"]
eks_desired_size        = 5
eks_min_size            = 3
eks_max_size            = 20
eks_disk_size           = 100

# RDS: multi-AZ, 100GB+ storage, 30-day backups, deletion protected
rds_instance_class        = "db.r6g.xlarge"
rds_allocated_storage     = 100
rds_multi_az              = true
rds_backup_retention_days = 30
rds_deletion_protection   = true

# Redis: 3 shards, 2 replicas each, auto-failover, 30-day backups
redis_node_type          = "cache.r6g.xlarge"
redis_cluster_enabled    = true
redis_num_shards         = 3
redis_replicas_per_shard = 2
redis_automatic_failover = true
redis_backup_retention_days = 30

# Monitoring: full stack with ingress
monitoring_enable_ingress      = true
monitoring_prometheus_retention = "30d"
monitoring_prometheus_storage   = "100Gi"
monitoring_grafana_storage      = "20Gi"
monitoring_grafana_admin_user   = "admin"
