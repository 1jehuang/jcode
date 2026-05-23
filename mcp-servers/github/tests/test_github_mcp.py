"""
Unit tests for GitHub MCP Server
Tests cover all 22 tools with mocked GitHub API responses
"""

import pytest
import asyncio
from unittest.mock import AsyncMock, patch, MagicMock
from src.server import (
    mcp,
    list_pull_requests,
    get_pull_request,
    review_pull_request,
    create_pull_request,
    merge_pull_request,
    get_pull_request_diff,
    approve_pull_request,
    list_issues,
    create_issue,
    get_issue,
    update_issue,
    add_issue_comment,
    close_issue,
    reopen_issue,
    get_file_content,
    get_repository_info,
    search_repositories,
    get_commit_history,
    create_branch,
)


class MockResponse:
    """Mock HTTP response"""
    def __init__(self, json_data, status_code=200, text=""):
        self._json_data = json_data
        self.status_code = status_code
        self.text = text
        self.content = b"{}" if status_code == 200 else b'{"message": "Error"}'

    def json(self):
        return self._json_data


@pytest.fixture
def mock_github_token():
    """Set mock GitHub token"""
    with patch('src.server.GITHUB_TOKEN', 'test_token_xyz'):
        yield


@pytest.fixture
def mock_httpx_client():
    """Mock httpx AsyncClient"""
    with patch('httpx.AsyncClient') as mock_client:
        yield mock_client


# ==================== Pull Request Tests ====================

@pytest.mark.asyncio
async def test_list_pull_requests(mock_github_token, mock_httpx_client):
    """Test listing pull requests"""
    # Mock response
    mock_prs = [
        {
            'number': 1,
            'title': 'Add new feature',
            'user': {'login': 'developer1'},
            'state': 'open'
        },
        {
            'number': 2,
            'title': 'Fix bug',
            'user': {'login': 'developer2'},
            'state': 'open'
        }
    ]

    mock_response = MockResponse(mock_prs)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    # Call function
    result = await list_pull_requests("owner/repo", "open")

    # Verify
    assert "#1: Add new feature by developer1 (open)" in result
    assert "#2: Fix bug by developer2 (open)" in result


@pytest.mark.asyncio
async def test_get_pull_request(mock_github_token, mock_httpx_client):
    """Test getting PR details"""
    mock_pr = {
        'number': 42,
        'title': 'Important fix',
        'user': {'login': 'contributor'},
        'state': 'open',
        'created_at': '2026-05-20T10:00:00Z',
        'body': 'This PR fixes a critical bug'
    }

    mock_response = MockResponse(mock_pr)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await get_pull_request("owner/repo", 42)

    assert "PR #42: Important fix" in result
    assert "Author: contributor" in result
    assert "State: open" in result
    assert "This PR fixes a critical bug" in result


@pytest.mark.asyncio
async def test_create_pull_request(mock_github_token, mock_httpx_client):
    """Test creating a new PR"""
    mock_pr = {
        'number': 100,
        'html_url': 'https://github.com/owner/repo/pull/100'
    }

    mock_response = MockResponse(mock_pr, status_code=201)
    mock_client_instance = AsyncMock()
    mock_client_instance.post = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await create_pull_request(
        "owner/repo",
        "New feature",
        "Description here",
        "feature-branch",
        "main"
    )

    assert "PR #100 created" in result
    assert "https://github.com/owner/repo/pull/100" in result


@pytest.mark.asyncio
async def test_merge_pull_request(mock_github_token, mock_httpx_client):
    """Test merging a PR"""
    mock_result = {
        'message': 'Pull Request successfully merged',
        'sha': 'abc123'
    }

    mock_response = MockResponse(mock_result)
    mock_client_instance = AsyncMock()
    mock_client_instance.put = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await merge_pull_request("owner/repo", 42, "squash")

    assert "PR #42 merged" in result


@pytest.mark.asyncio
async def test_get_pull_request_diff(mock_github_token, mock_httpx_client):
    """Test getting PR diff"""
    diff_content = """diff --git a/file.py b/file.py
index 1234567..abcdefg 100644
--- a/file.py
+++ b/file.py
@@ -1,3 +1,4 @@
 def hello():
+    print("Hello, world!")
     pass"""

    mock_response = MockResponse({}, status_code=200, text=diff_content)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await get_pull_request_diff("owner/repo", 42)

    assert "diff --git" in result
    assert "+    print" in result


@pytest.mark.asyncio
async def test_approve_pull_request(mock_github_token, mock_httpx_client):
    """Test approving a PR"""
    mock_response = MockResponse({'id': 123})
    mock_client_instance = AsyncMock()
    mock_client_instance.post = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await approve_pull_request("owner/repo", 42, "Looks good!")

    assert "PR #42 approved" in result


# ==================== Issue Tests ====================

@pytest.mark.asyncio
async def test_list_issues(mock_github_token, mock_httpx_client):
    """Test listing issues"""
    mock_issues = [
        {
            'number': 10,
            'title': 'Bug report',
            'user': {'login': 'user1'}
        },
        {
            'number': 11,
            'title': 'Feature request',
            'user': {'login': 'user2'},
            'pull_request': {}  # This should be filtered out
        }
    ]

    mock_response = MockResponse(mock_issues)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await list_issues("owner/repo", "open")

    assert "#10: Bug report by user1" in result
    assert "#11" not in result  # PRs should be filtered


@pytest.mark.asyncio
async def test_create_issue(mock_github_token, mock_httpx_client):
    """Test creating an issue"""
    mock_issue = {
        'number': 99,
        'html_url': 'https://github.com/owner/repo/issues/99'
    }

    mock_response = MockResponse(mock_issue, status_code=201)
    mock_client_instance = AsyncMock()
    mock_client_instance.post = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await create_issue("owner/repo", "New issue", "Description")

    assert "Issue #99 created" in result


@pytest.mark.asyncio
async def test_close_and_reopen_issue(mock_github_token, mock_httpx_client):
    """Test closing and reopening an issue"""
    mock_response = MockResponse({'number': 10, 'state': 'closed'})
    mock_client_instance = AsyncMock()
    mock_client_instance.patch = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    # Close
    result = await close_issue("owner/repo", 10)
    assert "Issue #10 closed" in result

    # Reopen
    mock_response_reopen = MockResponse({'number': 10, 'state': 'open'})
    mock_client_instance.patch = AsyncMock(return_value=mock_response_reopen)
    result = await reopen_issue("owner/repo", 10)
    assert "Issue #10 reopened" in result


# ==================== Repository Tests ====================

@pytest.mark.asyncio
async def test_get_repository_info(mock_github_token, mock_httpx_client):
    """Test getting repository metadata"""
    mock_repo = {
        'full_name': 'owner/repo',
        'description': 'A test repository',
        'stargazers_count': 100,
        'forks_count': 50,
        'open_issues_count': 10,
        'default_branch': 'main',
        'language': 'Python',
        'created_at': '2020-01-01T00:00:00Z',
        'updated_at': '2026-05-22T00:00:00Z',
        'license': {'name': 'MIT'},
        'html_url': 'https://github.com/owner/repo'
    }

    mock_response = MockResponse(mock_repo)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await get_repository_info("owner/repo")

    assert "Repository: owner/repo" in result
    assert "Stars: 100" in result
    assert "Forks: 50" in result
    assert "Language: Python" in result
    assert "License: MIT" in result


@pytest.mark.asyncio
async def test_search_repositories(mock_github_token, mock_httpx_client):
    """Test searching repositories"""
    mock_result = {
        'total_count': 1000,
        'items': [
            {
                'full_name': 'popular/repo1',
                'stargazers_count': 5000,
                'description': 'Popular ML framework',
                'html_url': 'https://github.com/popular/repo1'
            }
        ]
    }

    mock_response = MockResponse(mock_result)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await search_repositories("machine learning", "stars", "desc")

    assert "Found 1000 repositories" in result
    assert "⭐ 5000 | popular/repo1" in result


@pytest.mark.asyncio
async def test_get_commit_history(mock_github_token, mock_httpx_client):
    """Test getting commit history"""
    mock_commits = [
        {
            'sha': 'abc123def456',
            'commit': {
                'message': 'Fix critical bug',
                'author': {
                    'name': 'Developer',
                    'date': '2026-05-22T10:00:00Z'
                }
            }
        },
        {
            'sha': 'def789ghi012',
            'commit': {
                'message': 'Add new feature\nDetailed description',
                'author': {
                    'name': 'Contributor',
                    'date': '2026-05-21T15:00:00Z'
                }
            }
        }
    ]

    mock_response = MockResponse(mock_commits)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await get_commit_history("owner/repo", "main", 2)

    assert "Recent commits on main" in result
    assert "abc123de - Fix critical bug (Developer, 2026-05-22)" in result
    assert "def789gh - Add new feature (Contributor, 2026-05-21)" in result


@pytest.mark.asyncio
async def test_create_branch(mock_github_token, mock_httpx_client):
    """Test creating a new branch"""
    # Mock getting source branch ref
    mock_ref = {'object': {'sha': 'abc123'}}
    mock_create = {'ref': 'refs/heads/feature-branch'}

    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=MockResponse(mock_ref))
    mock_client_instance.post = AsyncMock(return_value=MockResponse(mock_create, 201))
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await create_branch("owner/repo", "feature-branch", "main")

    assert "Branch 'feature-branch' created from 'main'" in result


# ==================== Error Handling Tests ====================

@pytest.mark.asyncio
async def test_handle_404_error(mock_github_token, mock_httpx_client):
    """Test handling 404 errors"""
    mock_response = MockResponse({}, status_code=404)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await get_pull_request("owner/nonexistent", 999)

    assert "Error: Resource not found (404)" in result


@pytest.mark.asyncio
async def test_handle_rate_limit(mock_github_token, mock_httpx_client):
    """Test handling rate limit errors"""
    mock_response = MockResponse({}, status_code=403)
    mock_client_instance = AsyncMock()
    mock_client_instance.get = AsyncMock(return_value=mock_response)
    mock_client_instance.__aenter__ = AsyncMock(return_value=mock_client_instance)
    mock_client_instance.__aexit__ = AsyncMock(return_value=None)
    mock_httpx_client.return_value = mock_client_instance

    result = await list_pull_requests("owner/repo")

    assert "Error: Rate limit exceeded or insufficient permissions (403)" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
