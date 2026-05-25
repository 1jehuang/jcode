output "prometheus_url" {
  description = "Prometheus service URL within the cluster"
  value       = "http://carpai-monitoring-prometheus.${var.project_name}-${var.environment}.svc.cluster.local:9090"
}

output "grafana_url" {
  description = "Grafana service URL within the cluster"
  value       = "http://carpai-monitoring-grafana.${var.project_name}-${var.environment}.svc.cluster.local:3000"
}

output "grafana_admin_secret_arn" {
  description = "ARN of Secrets Manager secret containing Grafana admin password"
  value       = aws_secretsmanager_secret.grafana_admin.arn
}

output "alertmanager_url" {
  description = "Alertmanager service URL within the cluster"
  value       = "http://carpai-monitoring-alertmanager.${var.project_name}-${var.environment}.svc.cluster.local:9093"
}
