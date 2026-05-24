# AWS MCP Server

MCP server for AWS cloud operations.

## Features

- **12+ Tools** for AWS services
- EC2 instance management
- S3 bucket operations
- Lambda function invocation
- CloudWatch metrics
- IAM user management

## Configuration

| Variable | Description |
|----------|-------------|
| `AWS_ACCESS_KEY_ID` | AWS access key |
| `AWS_SECRET_ACCESS_KEY` | AWS secret key |
| `AWS_DEFAULT_REGION` | AWS region (us-east-1) |

## Available Tools

1. `list_ec2_instances()` - List EC2 instances
2. `start_ec2_instance(instance_id)` - Start instance
3. `stop_ec2_instance(instance_id)` - Stop instance
4. `list_s3_buckets()` - List S3 buckets
5. `list_s3_objects(bucket)` - List bucket objects
6. `upload_to_s3(bucket, key, content)` - Upload file
7. `invoke_lambda(function_name, payload)` - Invoke Lambda
8. `get_cloudwatch_metrics(namespace)` - Get metrics
9. `list_iam_users()` - List IAM users
10. `get_cost_estimate(service)` - Get cost estimate

## Testing

```bash
pytest tests/test_aws_mcp.py -v
```

Requires AWS credentials with appropriate permissions.

## License

MIT
