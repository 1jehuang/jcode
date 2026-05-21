"""
JCode Python SDK

This SDK provides access to the JCode AI Code Assistant API, enabling:
- Code completion
- CRDT-based collaborative editing
- SSO authentication
- Real-time collaboration via WebSocket

Example:
    from jcode import JCodeClient
    
    client = JCodeClient(base_url="http://localhost:8080")
    completions = client.complete("def hello():", "python")
    print(completions)
"""

from .client import JCodeClient
from .completion import CompletionClient, CompletionCandidate, CompletionContext
from .crdt import CrdtClient, CrdtDocument, EditOperation
from .sso import SsoClient, SsoUserInfo
from .models import SessionInfo, ProviderInfo, MetricsData

__version__ = "0.12.0"
__all__ = [
    "JCodeClient",
    "CompletionClient",
    "CompletionCandidate",
    "CompletionContext",
    "CrdtClient",
    "CrdtDocument",
    "EditOperation",
    "SsoClient",
    "SsoUserInfo",
    "SessionInfo",
    "ProviderInfo",
    "MetricsData",
]