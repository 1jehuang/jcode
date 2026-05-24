"""Docker MCP Server - Unit Tests"""
import pytest
from unittest.mock import MagicMock, patch


class TestDockerMCP:
    @pytest.fixture
    def mock_container(self):
        container = MagicMock()
        container.name = "test-container"
        container.status = "running"
        container.image.tags = ["nginx:latest"]
        return container

    @pytest.fixture
    def mock_client(self, mock_container):
        client = MagicMock()
        client.containers.list.return_value = [mock_container]
        client.containers.get.return_value = mock_container
        return client

    @pytest.mark.asyncio
    async def test_list_containers(self, mock_client):
        """Test listing containers"""
        with patch('src.server.get_client', return_value=mock_client):
            from src.server import list_containers
            result = await list_containers()

        assert "test-container" in result

    @pytest.mark.asyncio
    async def test_start_container(self, mock_client, mock_container):
        """Test starting a container"""
        with patch('src.server.get_client', return_value=mock_client):
            from src.server import start_container
            result = await start_container("test-container")

        assert "started" in result
        mock_container.start.assert_called_once()

    @pytest.mark.asyncio
    async def test_stop_container(self, mock_client, mock_container):
        """Test stopping a container"""
        with patch('src.server.get_client', return_value=mock_client):
            from src.server import stop_container
            result = await stop_container("test-container")

        assert "stopped" in result
        mock_container.stop.assert_called_once()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
