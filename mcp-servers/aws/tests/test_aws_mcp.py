"""AWS MCP Server - Unit Tests"""
import pytest
from unittest.mock import MagicMock, patch


class TestAWSMCP:
    @pytest.fixture
    def mock_ec2(self):
        ec2 = MagicMock()
        instance = {"InstanceId": "i-123", "State": {"Name": "running"}}
        ec2.describe_instances.return_value = {"Reservations": [{"Instances": [instance]}]}
        return ec2

    @pytest.mark.asyncio
    async def test_list_ec2_instances(self, mock_ec2):
        """Test listing EC2 instances"""
        with patch('boto3.client', return_value=mock_ec2):
            from src.server import list_ec2_instances
            result = await list_ec2_instances()

        assert "i-123" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
