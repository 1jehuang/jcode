"""Jira MCP Server - Unit Tests"""
import pytest
from unittest.mock import AsyncMock, patch


class TestJiraMCP:
    @pytest.fixture
    def mock_client(self):
        return AsyncMock()

    @pytest.mark.asyncio
    async def test_search_issues(self, mock_client):
        """Test JQL search"""
        mock_response = AsyncMock()
        mock_response.json.return_value = {
            "issues": [
                {"key": "PROJ-1", "fields": {"summary": "Bug 1", "status": {"name": "Open"}}}
            ]
        }
        mock_client.get.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import search_issues
            result = await search_issues("project = PROJ")

        assert "PROJ-1" in result

    @pytest.mark.asyncio
    async def test_get_issue(self, mock_client):
        """Test getting issue details"""
        mock_response = AsyncMock()
        mock_response.json.return_value = {
            "key": "PROJ-123",
            "fields": {
                "summary": "Test Issue",
                "status": {"name": "In Progress"},
                "priority": {"name": "High"},
                "assignee": {"displayName": "Alice"},
                "reporter": {"displayName": "Bob"},
                "created": "2026-05-24T00:00:00.000+0000",
                "description": "Test description"
            }
        }
        mock_client.get.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import get_issue
            result = await get_issue("PROJ-123")

        assert "PROJ-123" in result
        assert "Test Issue" in result

    @pytest.mark.asyncio
    async def test_create_issue(self, mock_client):
        """Test creating an issue"""
        mock_response = AsyncMock()
        mock_response.json.return_value = {"key": "PROJ-456"}
        mock_client.post.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import create_issue
            result = await create_issue("PROJ", "New Bug", "Bug")

        assert "PROJ-456" in result

    @pytest.mark.asyncio
    async def test_delete_issue_requires_confirm(self):
        """Test delete requires confirmation"""
        from src.server import delete_issue
        result = await delete_issue("PROJ-123", confirm=False)

        assert "WARNING" in result

    @pytest.mark.asyncio
    async def test_add_comment(self, mock_client):
        """Test adding a comment"""
        mock_response = AsyncMock()
        mock_client.post.return_value = mock_response

        with patch('httpx.AsyncClient') as mock_httpx:
            mock_httpx.return_value.__aenter__.return_value = mock_client
            from src.server import add_comment
            result = await add_comment("PROJ-123", "Fixed!")

        assert "Comment added" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
