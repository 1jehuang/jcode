"""
SSO authentication client
"""

import requests
from typing import Optional, List, Dict, Any
from .models import SsoUserInfo, SsoProviderConfig


class SsoClient:
    """
    Client for SSO authentication
    
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
    
    def list_providers(self) -> List[SsoProviderConfig]:
        """
        List all SSO providers
        
        :return: List of SsoProviderConfig objects
        """
        url = f"{self.base_url}/api/v1/sso/providers"
        
        response = self._session.get(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return [SsoProviderConfig(**item) for item in response.json()]
    
    def get_provider(self, provider_id: str) -> SsoProviderConfig:
        """
        Get provider by ID
        
        :param provider_id: Provider ID
        
        :return: SsoProviderConfig object
        """
        url = f"{self.base_url}/api/v1/sso/providers/{provider_id}"
        
        response = self._session.get(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return SsoProviderConfig(**response.json())
    
    def create_provider(self, config: SsoProviderConfig) -> SsoProviderConfig:
        """
        Create a new SSO provider
        
        :param config: Provider configuration
        
        :return: Created SsoProviderConfig object
        """
        url = f"{self.base_url}/api/v1/sso/providers"
        
        response = self._session.post(
            url,
            json=config.dict(),
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return SsoProviderConfig(**response.json())
    
    def update_provider(self, provider_id: str, config: SsoProviderConfig) -> SsoProviderConfig:
        """
        Update provider configuration
        
        :param provider_id: Provider ID
        :param config: Updated configuration
        
        :return: Updated SsoProviderConfig object
        """
        url = f"{self.base_url}/api/v1/sso/providers/{provider_id}"
        
        response = self._session.put(
            url,
            json=config.dict(),
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return SsoProviderConfig(**response.json())
    
    def delete_provider(self, provider_id: str) -> None:
        """
        Delete a provider
        
        :param provider_id: Provider ID
        """
        url = f"{self.base_url}/api/v1/sso/providers/{provider_id}"
        
        response = self._session.delete(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
    
    def get_authorization_url(self, provider_id: str, redirect_uri: str) -> str:
        """
        Get authorization URL for a provider
        
        :param provider_id: Provider ID
        :param redirect_uri: Redirect URI
        
        :return: Authorization URL
        """
        url = f"{self.base_url}/api/v1/sso/providers/{provider_id}/authorize"
        
        params = {
            "redirect_uri": redirect_uri,
        }
        
        response = self._session.get(
            url,
            params=params,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return response.json().get("url")
    
    def exchange_code(self, provider_id: str, code: str, redirect_uri: str) -> Dict[str, Any]:
        """
        Exchange authorization code for tokens
        
        :param provider_id: Provider ID
        :param code: Authorization code
        :param redirect_uri: Redirect URI
        
        :return: Token response dictionary
        """
        url = f"{self.base_url}/api/v1/sso/providers/{provider_id}/token"
        
        data = {
            "code": code,
            "redirect_uri": redirect_uri,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return response.json()
    
    def get_user_info(self, provider_id: str, access_token: str) -> SsoUserInfo:
        """
        Get user information from provider
        
        :param provider_id: Provider ID
        :param access_token: Access token
        
        :return: SsoUserInfo object
        """
        url = f"{self.base_url}/api/v1/sso/providers/{provider_id}/userinfo"
        
        headers = self._get_headers()
        headers["Authorization"] = f"Bearer {access_token}"
        
        response = self._session.get(
            url,
            headers=headers,
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return SsoUserInfo(**response.json())
    
    def validate_token(self, provider_id: str, token: str) -> SsoUserInfo:
        """
        Validate access token and get user info
        
        :param provider_id: Provider ID
        :param token: Access token
        
        :return: SsoUserInfo object
        """
        url = f"{self.base_url}/api/v1/sso/validate"
        
        data = {
            "provider_id": provider_id,
            "token": token,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return SsoUserInfo(**response.json())


__all__ = ["SsoClient", "SsoUserInfo", "SsoProviderConfig"]