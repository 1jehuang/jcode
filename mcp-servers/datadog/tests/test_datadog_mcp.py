"""Datadog MCP Server - Unit Tests"""
import pytest
from unittest.mock import AsyncMock, patch


class TestDatadogMCP:
    @pytest.fixture
    def mock_client(self):
        return AsyncMock()

    @pytest.mark.asyncio
    async def test_query_metrics(self, mock_client):
        """Test querying Datadog metrics"""
        mock_response = AsyncMock()
        mock_response.json.return_value = {
            "series": [{"metric": "cpu.usage", "points": [[100, 50.5]]}]
        }
        mock_client.get.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import query_metrics
            result = await query_metrics("avg:cpu.usage", 1000000, 2000000)

        assert "cpu.usage" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
