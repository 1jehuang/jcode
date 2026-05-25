output "vpc_id" {
  description = "VPC ID"
  value       = module.networking.vpc_id
}

output "eks_cluster_id" {
  description = "EKS cluster ID"
  value       = module.eks.cluster_id
}

output "eks_cluster_endpoint" {
  description = "EKS cluster endpoint URL"
  value       = module.eks.cluster_endpoint
}

output "rds_endpoint" {
  description = "RDS PostgreSQL endpoint"
  value       = module.rds.endpoint
}

output "rds_database_name" {
  description = "RDS database name"
  value       = module.rds.database_name
}

output "rds_master_password_secret_arn" {
  description = "ARN of Secrets Manager secret containing RDS master password"
  value       = module.rds.master_password_secret_arn
}

output "redis_primary_endpoint" {
  description = "Redis primary endpoint"
  value       = module.redis.primary_endpoint
}

output "redis_reader_endpoint" {
  description = "Redis reader endpoint"
  value       = module.redis.reader_endpoint
}

output "monitoring_grafana_url" {
  description = "Grafana URL"
  value       = module.monitoring.grafana_url
}

output "monitoring_grafana_admin_secret_arn" {
  description = "ARN of Secrets Manager secret containing Grafana admin password"
  value       = module.monitoring.grafana_admin_secret_arn
}

output "monitoring_prometheus_url" {
  description = "Prometheus URL"
  value       = module.monitoring.prometheus_url
}
