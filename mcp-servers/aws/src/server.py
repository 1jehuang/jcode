"""
AWS MCP Server
==============
Provides: EC2, S3, Lambda, CloudWatch, ECS, RDS, and Security Groups management
via the Model Context Protocol (MCP) using FastMCP and boto3.
"""

from mcp.server import FastMCP
import boto3
import os
import json
from datetime import datetime
from typing import Optional

mcp = FastMCP("aws")


def _get_aws_session(region: str = "us-east-1"):
    return boto3.Session(
        aws_access_key_id=os.getenv("AWS_ACCESS_KEY_ID"),
        aws_secret_access_key=os.getenv("AWS_SECRET_ACCESS_KEY"),
        aws_session_token=os.getenv("AWS_SESSION_TOKEN"),
        region_name=region,
    )


def _serialize(obj):
    if isinstance(obj, dict):
        return {k: _serialize(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_serialize(v) for v in obj]
    if isinstance(obj, datetime):
        return obj.isoformat()
    if isinstance(obj, bytes):
        return obj.decode("utf-8", errors="replace")
    return obj


def _format_output(data, label: str = "Result") -> str:
    try:
        cleaned = _serialize(data)
        return json.dumps(cleaned, indent=2, ensure_ascii=False, default=str)
    except Exception as exc:
        return f"{label}: {data}\n(Serialisation warning: {exc})"


# ---------------------------------------------------------------------------
# 1. EC2: list instances
# ---------------------------------------------------------------------------
@mcp.tool()
def list_ec2_instances(region: str = "us-east-1") -> str:
    """List all EC2 instances with state, type, IPs and tags."""
    try:
        ec2 = _get_aws_session(region).client("ec2")
        resp = ec2.describe_instances()
        instances = []
        for reservation in resp.get("Reservations", []):
            for inst in reservation.get("Instances", []):
                instances.append({
                    "InstanceId": inst["InstanceId"],
                    "State": inst["State"]["Name"],
                    "InstanceType": inst["InstanceType"],
                    "LaunchTime": inst.get("LaunchTime"),
                    "PublicIp": inst.get("PublicIpAddress", "N/A"),
                    "PrivateIp": inst.get("PrivateIpAddress", "N/A"),
                    "VpcId": inst.get("VpcId", "N/A"),
                    "Tags": {t["Key"]: t["Value"] for t in inst.get("Tags", [])},
                })
        if not instances:
            return f"No EC2 instances found in region {region}"
        return _format_output(instances, "EC2 Instances")
    except Exception as e:
        return f"Error listing EC2 instances: {e}"


# ---------------------------------------------------------------------------
# 2. EC2: describe single instance
# ---------------------------------------------------------------------------
@mcp.tool()
def describe_ec2_instance(instance_id: str, region: str = "us-east-1") -> str:
    """Retrieve full details for a specific EC2 instance."""
    try:
        ec2 = _get_aws_session(region).client("ec2")
        resp = ec2.describe_instances(InstanceIds=[instance_id])
        instances = resp.get("Reservations", [{}])[0].get("Instances", [])
        if not instances:
            return f"Instance {instance_id} not found in region {region}"
        return _format_output(instances[0], f"EC2 Instance {instance_id}")
    except Exception as e:
        return f"Error describing EC2 instance: {e}"


# ---------------------------------------------------------------------------
# 3. S3: list buckets
# ---------------------------------------------------------------------------
@mcp.tool()
def list_s3_buckets() -> str:
    """List all S3 buckets in the account."""
    try:
        s3 = _get_aws_session().client("s3")
        resp = s3.list_buckets()
        buckets = []
        for bucket in resp.get("Buckets", []):
            buckets.append({
                "Name": bucket["Name"],
                "CreationDate": bucket.get("CreationDate"),
            })
        if not buckets:
            return "No S3 buckets found"
        return _format_output(buckets, "S3 Buckets")
    except Exception as e:
        return f"Error listing S3 buckets: {e}"


# ---------------------------------------------------------------------------
# 4. S3: list objects
# ---------------------------------------------------------------------------
@mcp.tool()
def list_s3_objects(bucket: str, prefix: str = "", max_keys: int = 50) -> str:
    """List objects in an S3 bucket with optional prefix filter."""
    try:
        s3 = _get_aws_session().client("s3")
        params = {"Bucket": bucket, "MaxKeys": max_keys}
        if prefix:
            params["Prefix"] = prefix
        resp = s3.list_objects_v2(**params)
        objects = resp.get("Contents", [])
        if not objects:
            return f"No objects found in bucket '{bucket}'" + (f" with prefix '{prefix}'" if prefix else "")
        result = []
        for obj in objects:
            result.append({
                "Key": obj["Key"],
                "Size": obj["Size"],
                "LastModified": obj.get("LastModified"),
                "StorageClass": obj.get("StorageClass", "STANDARD"),
            })
        return _format_output(result, f"S3 Objects in {bucket}")
    except Exception as e:
        return f"Error listing S3 objects: {e}"


# ---------------------------------------------------------------------------
# 5. Lambda: list functions
# ---------------------------------------------------------------------------
@mcp.tool()
def list_lambda_functions(region: str = "us-east-1") -> str:
    """List Lambda functions in the specified region."""
    try:
        lam = _get_aws_session(region).client("lambda")
        resp = lam.list_functions(MaxItems=50)
        functions = []
        for fn in resp.get("Functions", []):
            functions.append({
                "FunctionName": fn["FunctionName"],
                "Runtime": fn.get("Runtime", "N/A"),
                "Handler": fn.get("Handler", "N/A"),
                "MemorySize": fn.get("MemorySize", "N/A"),
                "Timeout": fn.get("Timeout", "N/A"),
                "LastModified": fn.get("LastModified", "N/A"),
            })
        if not functions:
            return f"No Lambda functions found in region {region}"
        return _format_output(functions, "Lambda Functions")
    except Exception as e:
        return f"Error listing Lambda functions: {e}"


# ---------------------------------------------------------------------------
# 6. CloudWatch: get metrics
# ---------------------------------------------------------------------------
@mcp.tool()
def get_cloudwatch_metrics(
    namespace: str, metric_name: str, period: int = 300, region: str = "us-east-1", hours_back: int = 1
) -> str:
    """Get CloudWatch metric statistics for the specified metric."""
    try:
        cw = _get_aws_session(region).client("cloudwatch")
        end_time = datetime.utcnow()
        start_time = end_time.replace(hour=0, minute=0, second=0, microsecond=0)
        # Use the last N hours
        import datetime as dt_mod
        start_time = end_time - dt_mod.timedelta(hours=hours_back)
        resp = cw.get_metric_statistics(
            Namespace=namespace,
            MetricName=metric_name,
            StartTime=start_time,
            EndTime=end_time,
            Period=period,
            Statistics=["Average", "Maximum", "Minimum", "SampleCount"],
        )
        datapoints = resp.get("Datapoints", [])
        if not datapoints:
            return f"No data for {namespace}/{metric_name} in last {hours_back}h"
        datapoints.sort(key=lambda x: x["Timestamp"])
        result = []
        for dp in datapoints:
            result.append({
                "Timestamp": dp["Timestamp"],
                "Average": dp.get("Average"),
                "Maximum": dp.get("Maximum"),
                "Minimum": dp.get("Minimum"),
                "SampleCount": dp.get("SampleCount"),
                "Unit": dp.get("Unit", "Count"),
            })
        return _format_output(result, f"CloudWatch {namespace}/{metric_name}")
    except Exception as e:
        return f"Error getting CloudWatch metrics: {e}"


# ---------------------------------------------------------------------------
# 7. ECS: list clusters
# ---------------------------------------------------------------------------
@mcp.tool()
def list_ecs_clusters(region: str = "us-east-1") -> str:
    """List ECS clusters in the specified region."""
    try:
        ecs = _get_aws_session(region).client("ecs")
        resp = ecs.list_clusters()
        clusters = resp.get("clusterArns", [])
        if not clusters:
            return f"No ECS clusters found in region {region}"
        details = ecs.describe_clusters(clusters=clusters)
        result = []
        for cluster in details.get("clusters", []):
            result.append({
                "ClusterName": cluster["clusterName"],
                "Status": cluster.get("status", "N/A"),
                "RunningTasks": cluster.get("runningTasksCount", 0),
                "PendingTasks": cluster.get("pendingTasksCount", 0),
                "ActiveServices": cluster.get("activeServicesCount", 0),
                "RegisteredInstances": cluster.get("registeredContainerInstancesCount", 0),
            })
        return _format_output(result, "ECS Clusters")
    except Exception as e:
        return f"Error listing ECS clusters: {e}"


# ---------------------------------------------------------------------------
# 8. RDS: list instances
# ---------------------------------------------------------------------------
@mcp.tool()
def list_rds_instances(region: str = "us-east-1") -> str:
    """List RDS database instances in the specified region."""
    try:
        rds = _get_aws_session(region).client("rds")
        resp = rds.describe_db_instances()
        instances = resp.get("DBInstances", [])
        if not instances:
            return f"No RDS instances found in region {region}"
        result = []
        for inst in instances:
            result.append({
                "DBInstanceIdentifier": inst["DBInstanceIdentifier"],
                "DBInstanceClass": inst["DBInstanceClass"],
                "Engine": inst["Engine"],
                "EngineVersion": inst.get("EngineVersion", "N/A"),
                "DBInstanceStatus": inst["DBInstanceStatus"],
                "Endpoint": inst.get("Endpoint", {}).get("Address", "N/A"),
                "MultiAZ": inst.get("MultiAZ", False),
                "Storage": f"{inst.getAllocatedStorage} GB" if inst.get("AllocatedStorage") else "N/A",
            })
        return _format_output(result, "RDS Instances")
    except Exception as e:
        return f"Error listing RDS instances: {e}"


# ---------------------------------------------------------------------------
# 9. Security Groups: list
# ---------------------------------------------------------------------------
@mcp.tool()
def list_security_groups(region: str = "us-east-1") -> str:
    """List security groups in the specified region."""
    try:
        ec2 = _get_aws_session(region).client("ec2")
        resp = ec2.describe_security_groups()
        sgs = resp.get("SecurityGroups", [])
        if not sgs:
            return f"No security groups found in region {region}"
        result = []
        for sg in sgs:
            result.append({
                "GroupId": sg["GroupId"],
                "GroupName": sg["GroupName"],
                "VpcId": sg.get("VpcId", "N/A"),
                "Description": sg.get("Description", ""),
                "InboundRules": len(sg.get("IpPermissions", [])),
                "OutboundRules": len(sg.get("IpPermissionsEgress", [])),
            })
        return _format_output(result, "Security Groups")
    except Exception as e:
        return f"Error listing security groups: {e}"


# ---------------------------------------------------------------------------
# 10. EC2 Instance Types
# ---------------------------------------------------------------------------
@mcp.tool()
def get_ec2_instance_types(region: str = "us-east-1", max_results: int = 30) -> str:
    """List available EC2 instance types in the specified region."""
    try:
        ec2 = _get_aws_session(region).client("ec2")
        resp = ec2.describe_instance_types(MaxResults=max_results)
        types = resp.get("InstanceTypes", [])
        if not types:
            return f"No instance types found in region {region}"
        result = []
        for it in types:
            result.append({
                "InstanceType": it["InstanceType"],
                "vCPUs": it.get("VCpuInfo", {}).get("DefaultVCpus", "N/A"),
                "MemoryMiB": it.get("MemoryInfo", {}).get("SizeInMiB", "N/A"),
                "NetworkPerformance": it.get("NetworkInfo", {}).get("NetworkPerformance", "N/A"),
            })
        return _format_output(result, "EC2 Instance Types")
    except Exception as e:
        return f"Error listing instance types: {e}"


if __name__ == "__main__":
    mcp.run()
