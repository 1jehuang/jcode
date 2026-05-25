output "cluster_endpoint" {
  description = "EKS cluster API endpoint"
  value       = module.carpai.cluster_endpoint
}

output "rds_endpoint" {
  description = "RDS PostgreSQL endpoint"
  value       = module.carpai.rds_endpoint
}

output "redis_endpoint" {
  description = "Redis primary endpoint"
  value       = module.carpai.redis_endpoint
}

output "grafana_url" {
  description = "Grafana URL"
  value       = module.carpai.grafana_url
}
