"""
Main JCode client
"""

import requests
from typing import Optional, Dict, Any
from .completion import CompletionClient
from .crdt import CrdtClient
from .sso import SsoClient


class JCodeClient:
    """
    Main client for interacting with JCode API
    
    :param base_url: JCode server base URL
    :param api_key: Optional API key for authentication
    :param timeout: Request timeout in seconds
    """
    
    def __init__(
        self,
        base_url: str = "http://localhost:8080",
        api_key: Optional[str] = None,
        timeout: int = 30,
    ):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout
        self._session = requests.Session()
        
        if api_key:
            self._session.headers.update({"Authorization": f"Bearer {api_key}"})
        
        # Initialize sub-clients
        self.completion = CompletionClient(base_url, api_key, timeout)
        self.crdt = CrdtClient(base_url, api_key, timeout)
        self.sso = SsoClient(base_url, api_key, timeout)
    
    def _get_headers(self) -> Dict[str, str]:
        """Get default headers"""
        headers = {
            "Content-Type": "application/json",
        }
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers
    
    def health_check(self) -> Dict[str, Any]:
        """
        Check server health
        
        :return: Health status
        """
        url = f"{self.base_url}/health"
        response = self._session.get(url, timeout=self.timeout)
        response.raise_for_status()
        return response.json()
    
    def get_version(self) -> Dict[str, Any]:
        """
        Get server version
        
        :return: Version information
        """
        url = f"{self.base_url}/version"
        response = self._session.get(url, timeout=self.timeout)
        response.raise_for_status()
        return response.json()
    
    def complete(
        self,
        content: str,
        language: str,
        cursor_line: int = 0,
        cursor_column: int = 0,
        file_path: Optional[str] = None,
    ):
        """
        Get code completions
        
        :param content: Source code content
        :param language: Programming language
        :param cursor_line: Cursor line position
        :param cursor_column: Cursor column position
        :param file_path: Optional file path
        
        :return: List of completion candidates
        """
        return self.completion.complete(
            content=content,
            language=language,
            cursor_line=cursor_line,
            cursor_column=cursor_column,
            file_path=file_path,
        )
    
    def close(self):
        """Close the client session"""
        self._session.close()
    
    def __enter__(self):
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()


__all__ = ["JCodeClient"]