"""Kubernetes MCP Server - Unit Tests"""
import pytest
from unittest.mock import MagicMock, patch


class TestKubernetesMCP:
    @pytest.fixture
    def mock_v1(self):
        v1 = MagicMock()
        pod = MagicMock()
        pod.metadata.name = "test-pod"
        pod.status.phase = "Running"
        v1.list_namespaced_pod.return_value.items = [pod]
        return v1

    @pytest.mark.asyncio
    async def test_list_pods(self, mock_v1):
        """Test listing pods"""
        with patch('kubernetes.client.CoreV1Api', return_value=mock_v1):
            from src.server import list_pods
            result = await list_pods("default")

        assert "test-pod" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
