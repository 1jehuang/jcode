"""
Data models for JCode SDK
"""

from typing import Optional, List, Dict, Any
from datetime import datetime
from pydantic import BaseModel, Field


class SessionInfo(BaseModel):
    """Session information"""
    session_id: str = Field(description="Unique session identifier")
    user_id: Optional[str] = Field(None, description="User ID")
    provider_id: Optional[str] = Field(None, description="Provider ID")
    created_at: datetime = Field(description="Creation timestamp")
    expires_at: datetime = Field(description="Expiration timestamp")
    is_active: bool = Field(description="Session active status")

    class Config:
        json_encoders = {
            datetime: lambda v: v.isoformat(),
        }


class ProviderInfo(BaseModel):
    """Provider information"""
    id: str = Field(description="Provider ID")
    name: str = Field(description="Provider name")
    provider_type: str = Field(description="Provider type (openai, anthropic, etc.)")
    enabled: bool = Field(description="Is provider enabled")
    model: Optional[str] = Field(None, description="Default model name")


class MetricsData(BaseModel):
    """Metrics data point"""
    timestamp: datetime = Field(description="Metric timestamp")
    name: str = Field(description="Metric name")
    value: float = Field(description="Metric value")
    labels: Dict[str, str] = Field(default_factory=dict, description="Metric labels")

    class Config:
        json_encoders = {
            datetime: lambda v: v.isoformat(),
        }


class CompletionContext(BaseModel):
    """Completion context"""
    file_path: str = Field(description="File path")
    line: int = Field(description="Cursor line")
    column: int = Field(description="Cursor column")
    prefix: str = Field(description="Text before cursor")
    expected_type: Optional[str] = Field(None, description="Expected type")
    scope: str = Field(description="Scope kind")
    parent_symbol: Optional[str] = Field(None, description="Parent symbol")


class CompletionCandidate(BaseModel):
    """Completion candidate"""
    label: str = Field(description="Completion label")
    kind: str = Field(description="Completion kind")
    detail: Optional[str] = Field(None, description="Detailed description")
    documentation: Optional[str] = Field(None, description="Documentation")
    insert_text: str = Field(description="Text to insert")
    rank_score: float = Field(description="Ranking score")
    is_multiline: bool = Field(False, description="Is multiline completion")


class EditOperation(BaseModel):
    """CRDT edit operation"""
    operation_id: str = Field(description="Unique operation ID")
    document_id: str = Field(description="Document ID")
    client_id: str = Field(description="Client ID")
    position: int = Field(description="Edit position")
    content: str = Field(description="Content to insert")
    delete_length: int = Field(0, description="Number of characters to delete")
    timestamp: datetime = Field(description="Operation timestamp")
    version: str = Field(description="Version identifier")

    class Config:
        json_encoders = {
            datetime: lambda v: v.isoformat(),
        }


class CrdtDocumentInfo(BaseModel):
    """CRDT document information"""
    document_id: str = Field(description="Document ID")
    title: str = Field(description="Document title")
    content: str = Field(description="Document content")
    version: str = Field(description="Current version")
    last_modified: datetime = Field(description="Last modified timestamp")
    collaborators: List[str] = Field(default_factory=list, description="Active collaborators")

    class Config:
        json_encoders = {
            datetime: lambda v: v.isoformat(),
        }


class SsoUserInfo(BaseModel):
    """SSO user information"""
    sub: str = Field(description="Subject ID")
    email: Optional[str] = Field(None, description="User email")
    email_verified: bool = Field(False, description="Email verified")
    name: Optional[str] = Field(None, description="Full name")
    nickname: Optional[str] = Field(None, description="Nickname")
    picture: Optional[str] = Field(None, description="Profile picture URL")
    tenant_id: Optional[str] = Field(None, description="Tenant ID")
    groups: List[str] = Field(default_factory=list, description="User groups")
    roles: List[str] = Field(default_factory=list, description="User roles")
    claims: Dict[str, str] = Field(default_factory=dict, description="Additional claims")


class SsoProviderConfig(BaseModel):
    """SSO provider configuration"""
    id: str = Field(description="Provider ID")
    name: str = Field(description="Provider name")
    provider_type: str = Field(description="Provider type (oidc, saml)")
    client_id: str = Field(description="Client ID")
    client_secret: Optional[str] = Field(None, description="Client secret")
    issuer_url: Optional[str] = Field(None, description="Issuer URL")
    discovery_url: Optional[str] = Field(None, description="Discovery URL")
    enabled: bool = Field(True, description="Is enabled")


class ErrorResponse(BaseModel):
    """Error response"""
    error: str = Field(description="Error type")
    message: str = Field(description="Error message")
    code: Optional[int] = Field(None, description="Error code")
    details: Optional[Dict[str, Any]] = Field(None, description="Additional details")