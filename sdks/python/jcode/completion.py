"""
Code completion client
"""

import requests
from typing import Optional, List, Dict, Any
from .models import CompletionCandidate, CompletionContext


class CompletionClient:
    """
    Client for code completion API
    
    :param base_url: JCode server base URL
    :param api_key: Optional API key
    :param timeout: Request timeout
    """
    
    def __init__(self, base_url: str, api_key: Optional[str] = None, timeout: int = 30):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout
        self._session = requests.Session()
        
        if api_key:
            self._session.headers.update({"Authorization": f"Bearer {api_key}"})
    
    def _get_headers(self) -> Dict[str, str]:
        """Get request headers"""
        headers = {
            "Content-Type": "application/json",
        }
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers
    
    def complete(
        self,
        content: str,
        language: str,
        cursor_line: int = 0,
        cursor_column: int = 0,
        file_path: Optional[str] = None,
    ) -> List[CompletionCandidate]:
        """
        Get code completions
        
        :param content: Source code content
        :param language: Programming language
        :param cursor_line: Cursor line position
        :param cursor_column: Cursor column position
        :param file_path: Optional file path
        
        :return: List of CompletionCandidate objects
        """
        url = f"{self.base_url}/api/v1/completions"
        
        data = {
            "content": content,
            "language": language,
            "cursor_line": cursor_line,
            "cursor_column": cursor_column,
            "file_path": file_path,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        result = response.json()
        return [CompletionCandidate(**item) for item in result.get("candidates", [])]
    
    def complete_stream(
        self,
        content: str,
        language: str,
        cursor_line: int = 0,
        cursor_column: int = 0,
        file_path: Optional[str] = None,
    ):
        """
        Stream code completions (async generator)
        
        :param content: Source code content
        :param language: Programming language
        :param cursor_line: Cursor line position
        :param cursor_column: Cursor column position
        :param file_path: Optional file path
        
        :yield: CompletionCandidate objects as they arrive
        """
        url = f"{self.base_url}/api/v1/completions/stream"
        
        data = {
            "content": content,
            "language": language,
            "cursor_line": cursor_line,
            "cursor_column": cursor_column,
            "file_path": file_path,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
            stream=True,
        )
        response.raise_for_status()
        
        for line in response.iter_lines():
            if line:
                import json
                item = json.loads(line)
                yield CompletionCandidate(**item)
    
    def get_context(
        self,
        content: str,
        cursor_line: int,
        cursor_column: int,
    ) -> CompletionContext:
        """
        Get completion context for a given cursor position
        
        :param content: Source code content
        :param cursor_line: Cursor line position
        :param cursor_column: Cursor column position
        
        :return: CompletionContext object
        """
        url = f"{self.base_url}/api/v1/completions/context"
        
        data = {
            "content": content,
            "cursor_line": cursor_line,
            "cursor_column": cursor_column,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return CompletionContext(**response.json())
    
    def get_stats(self) -> Dict[str, Any]:
        """
        Get completion statistics
        
        :return: Statistics dictionary
        """
        url = f"{self.base_url}/api/v1/completions/stats"
        
        response = self._session.get(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return response.json()


__all__ = ["CompletionClient", "CompletionCandidate", "CompletionContext"]