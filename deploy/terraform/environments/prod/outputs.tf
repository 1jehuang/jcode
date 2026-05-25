output "cluster_endpoint" {
  description = "EKS cluster API endpoint"
  value       = module.carpai.cluster_endpoint
}

output "cluster_oidc_provider" {
  description = "EKS OIDC provider ARN"
  value       = module.carpai.cluster_oidc_provider
}

output "rds_endpoint" {
  description = "RDS PostgreSQL endpoint"
  value       = module.carpai.rds_endpoint
}

output "rds_password_secret_arn" {
  description = "ARN of Secrets Manager secret for RDS password"
  value       = module.carpai.rds_password_secret_arn
}

output "redis_endpoint" {
  description = "Redis primary endpoint"
  value       = module.carpai.redis_endpoint
}

output "grafana_url" {
  description = "Grafana URL"
  value       = module.carpai.grafana_url
}

output "grafana_admin_secret_arn" {
  description = "ARN of Secrets Manager secret for Grafana admin password"
  value       = module.carpai.grafana_admin_secret_arn
}
