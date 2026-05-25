variable "environment" {
  description = "Environment name (dev/prod)"
  type        = string
}

variable "project_name" {
  description = "Project name for resource naming"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID for EKS cluster"
  type        = string
}

variable "subnet_ids" {
  description = "List of private subnet IDs for EKS nodes"
  type        = list(string)
}

variable "cluster_version" {
  description = "Kubernetes version for EKS cluster"
  type        = string
  default     = "1.28"
}

variable "instance_types" {
  description = "EC2 instance types for EKS node group"
  type        = list(string)
  default     = ["m6i.large"]
}

variable "min_size" {
  description = "Minimum node count for EKS node group"
  type        = number
  default     = 3
}

variable "max_size" {
  description = "Maximum node count for EKS node group"
  type        = number
  default     = 20
}

variable "desired_size" {
  description = "Desired node count for EKS node group"
  type        = number
  default     = 5
}

variable "disk_size" {
  description = "Disk size in GB for EKS nodes"
  type        = number
  default     = 80
}

variable "common_tags" {
  description = "Common tags for all resources"
  type        = map(string)
  default     = {}
}
