"""Slack MCP Server - Unit Tests"""
import pytest
from unittest.mock import AsyncMock, patch


class TestSlackMCP:
    @pytest.fixture
    def mock_client(self):
        return AsyncMock()

    @pytest.mark.asyncio
    async def test_send_message(self, mock_client):
        """Test sending a message"""
        mock_response = AsyncMock()
        mock_response.json.return_value = {"ok": True, "ts": "1234567890.123456"}
        mock_client.post.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import send_message
            result = await send_message("#general", "Hello!")

        assert "Message sent" in result

    @pytest.mark.asyncio
    async def test_send_message_failure(self, mock_client):
        """Test message send failure"""
        mock_response = AsyncMock()
        mock_response.json.return_value = {"ok": False, "error": "channel_not_found"}
        mock_client.post.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import send_message
            result = await send_message("#invalid", "Hello!")

        assert "Failed" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
