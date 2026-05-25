resource "random_password" "grafana_admin" {
  length  = 24
  special = false
}

resource "aws_secretsmanager_secret" "grafana_admin" {
  name = "${var.project_name}-${var.environment}-grafana-admin-password"

  tags = var.common_tags
}

resource "aws_secretsmanager_secret_version" "grafana_admin" {
  secret_id = aws_secretsmanager_secret.grafana_admin.id
  secret_string = jsonencode({
    password = coalesce(var.grafana_admin_password, random_password.grafana_admin.result)
  })
}

# ─────────────────────────────────────────────────────
# kube-prometheus-stack (Helm Release)
# ─────────────────────────────────────────────────────
resource "helm_release" "kube_prometheus_stack" {
  name             = "carpai-monitoring"
  namespace        = "${var.project_name}-${var.environment}"
  create_namespace = true

  repository = "https://prometheus-community.github.io/helm-charts"
  chart      = "kube-prometheus-stack"
  version    = "56.0.0"

  values = [
    templatefile("${path.module}/values.yaml.tpl", {
      prometheus_retention = var.prometheus_retention
      prometheus_storage   = var.prometheus_storage
      grafana_storage      = var.grafana_storage
      grafana_admin_user   = var.grafana_admin_user
      grafana_admin_password = coalesce(var.grafana_admin_password, random_password.grafana_admin.result)
      enable_ingress       = var.enable_ingress
      project_name         = var.project_name
      environment          = var.environment
    })
  ]

  # Wait for EKS to be ready before installing monitoring
  depends_on = [random_password.grafana_admin]
}

# Register Grafana admin password as a local value for the root module
# (Already exported via grafana_admin_secret_arn)
