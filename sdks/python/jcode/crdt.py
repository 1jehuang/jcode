"""
CRDT collaborative editing client
"""

import requests
import asyncio
import websockets
import json
from typing import Optional, List, Dict, Any, Callable
from .models import CrdtDocumentInfo, EditOperation


class CrdtClient:
    """
    Client for CRDT-based collaborative editing
    
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
        
        self._ws_url = base_url.replace("http://", "ws://").replace("https://", "wss://")
        self._ws_connection = None
        self._on_edit_callback = None
    
    def _get_headers(self) -> Dict[str, str]:
        """Get request headers"""
        headers = {
            "Content-Type": "application/json",
        }
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers
    
    def create_document(self, title: str, content: str = "") -> CrdtDocumentInfo:
        """
        Create a new CRDT document
        
        :param title: Document title
        :param content: Initial content
        
        :return: CrdtDocumentInfo object
        """
        url = f"{self.base_url}/api/v1/crdt/documents"
        
        data = {
            "title": title,
            "content": content,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return CrdtDocumentInfo(**response.json())
    
    def get_document(self, document_id: str) -> CrdtDocumentInfo:
        """
        Get document by ID
        
        :param document_id: Document ID
        
        :return: CrdtDocumentInfo object
        """
        url = f"{self.base_url}/api/v1/crdt/documents/{document_id}"
        
        response = self._session.get(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return CrdtDocumentInfo(**response.json())
    
    def update_document(self, document_id: str, title: Optional[str] = None) -> CrdtDocumentInfo:
        """
        Update document metadata
        
        :param document_id: Document ID
        :param title: Optional new title
        
        :return: Updated CrdtDocumentInfo object
        """
        url = f"{self.base_url}/api/v1/crdt/documents/{document_id}"
        
        data = {}
        if title:
            data["title"] = title
        
        response = self._session.patch(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return CrdtDocumentInfo(**response.json())
    
    def delete_document(self, document_id: str) -> None:
        """
        Delete a document
        
        :param document_id: Document ID
        """
        url = f"{self.base_url}/api/v1/crdt/documents/{document_id}"
        
        response = self._session.delete(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
    
    def list_documents(self) -> List[CrdtDocumentInfo]:
        """
        List all documents
        
        :return: List of CrdtDocumentInfo objects
        """
        url = f"{self.base_url}/api/v1/crdt/documents"
        
        response = self._session.get(
            url,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return [CrdtDocumentInfo(**item) for item in response.json()]
    
    def apply_edit(
        self,
        document_id: str,
        position: int,
        content: str,
        delete_length: int = 0,
    ) -> EditOperation:
        """
        Apply an edit operation to a document
        
        :param document_id: Document ID
        :param position: Edit position
        :param content: Content to insert
        :param delete_length: Number of characters to delete
        
        :return: EditOperation object
        """
        url = f"{self.base_url}/api/v1/crdt/documents/{document_id}/edit"
        
        data = {
            "position": position,
            "content": content,
            "delete_length": delete_length,
        }
        
        response = self._session.post(
            url,
            json=data,
            headers=self._get_headers(),
            timeout=self.timeout,
        )
        response.raise_for_status()
        
        return EditOperation(**response.json())
    
    async def connect_websocket(
        self,
        document_id: str,
        client_id: str,
        on_edit: Optional[Callable[[EditOperation], None]] = None,
    ):
        """
        Connect to WebSocket for real-time collaboration
        
        :param document_id: Document ID
        :param client_id: Client ID
        :param on_edit: Callback for received edits
        """
        self._on_edit_callback = on_edit
        ws_url = f"{self._ws_url}/api/v1/crdt/ws/{document_id}?client_id={client_id}"
        
        headers = {}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        
        async with websockets.connect(ws_url, extra_headers=headers) as websocket:
            self._ws_connection = websocket
            
            async for message in websocket:
                data = json.loads(message)
                if data.get("type") == "edit":
                    operation = EditOperation(**data.get("payload", {}))
                    if on_edit:
                        on_edit(operation)
    
    async def send_edit(
        self,
        document_id: str,
        position: int,
        content: str,
        delete_length: int = 0,
    ):
        """
        Send an edit via WebSocket
        
        :param document_id: Document ID
        :param position: Edit position
        :param content: Content to insert
        :param delete_length: Number of characters to delete
        """
        if not self._ws_connection:
            raise RuntimeError("WebSocket not connected")
        
        message = json.dumps({
            "type": "edit",
            "payload": {
                "document_id": document_id,
                "position": position,
                "content": content,
                "delete_length": delete_length,
            },
        })
        
        await self._ws_connection.send(message)
    
    def close_websocket(self):
        """Close WebSocket connection"""
        self._ws_connection = None
        self._on_edit_callback = None


__all__ = ["CrdtClient", "CrdtDocumentInfo", "EditOperation"]