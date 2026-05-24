"""Sentry MCP Server - Unit Tests"""
import pytest
from unittest.mock import AsyncMock, patch


class TestSentryMCP:
    @pytest.fixture
    def mock_client(self):
        return AsyncMock()

    @pytest.mark.asyncio
    async def test_list_issues(self, mock_client):
        """Test listing Sentry issues"""
        mock_response = AsyncMock()
        mock_response.json.return_value = [
            {"id": "123", "title": "Error 1", "status": "unresolved"}
        ]
        mock_client.get.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import list_issues
            result = await list_issues("my-project")

        assert "Error 1" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
