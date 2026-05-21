# JCode Python SDK

Python SDK for the JCode AI Code Assistant.

## Installation

```bash
pip install jcode-sdk
```

## Quick Start

```python
from jcode import JCodeClient

# Initialize client
client = JCodeClient(base_url="http://localhost:8080")

# Get code completions
completions = client.complete(
    content="def hello():",
    language="python",
    cursor_line=0,
    cursor_column=12
)

for completion in completions:
    print(f"{completion.label} - {completion.rank_score}")
```

## Features

### Code Completion

```python
from jcode import CompletionClient

client = CompletionClient(base_url="http://localhost:8080")

# Get completions
candidates = client.complete(
    content="def fibonacci(n):",
    language="python",
    cursor_line=0,
    cursor_column=16
)

# Stream completions
for candidate in client.complete_stream(
    content="class MyClass:",
    language="python"
):
    print(candidate.label)
```

### CRDT Collaboration

```python
from jcode import CrdtClient

client = CrdtClient(base_url="http://localhost:8080")

# Create document
doc = client.create_document("My Document", "Initial content")

# Apply edit
operation = client.apply_edit(
    document_id=doc.document_id,
    position=15,
    content=" edited"
)

# Get document
doc = client.get_document(doc.document_id)
print(doc.content)  # "Initial content edited"
```

### SSO Authentication

```python
from jcode import SsoClient

client = SsoClient(base_url="http://localhost:8080")

# List providers
providers = client.list_providers()

# Get authorization URL
auth_url = client.get_authorization_url(
    provider_id="my-provider",
    redirect_uri="http://localhost:3000/callback"
)

# Exchange code for token
tokens = client.exchange_code(
    provider_id="my-provider",
    code="authorization-code",
    redirect_uri="http://localhost:3000/callback"
)

# Get user info
user_info = client.get_user_info(
    provider_id="my-provider",
    access_token=tokens["access_token"]
)
```

## API Reference

### JCodeClient

- `health_check()` - Check server health
- `get_version()` - Get server version
- `complete(content, language, cursor_line=0, cursor_column=0, file_path=None)` - Get completions

### CompletionClient

- `complete(content, language, cursor_line, cursor_column, file_path)` - Get completions
- `complete_stream(content, language, cursor_line, cursor_column, file_path)` - Stream completions
- `get_context(content, cursor_line, cursor_column)` - Get completion context
- `get_stats()` - Get statistics

### CrdtClient

- `create_document(title, content)` - Create document
- `get_document(document_id)` - Get document
- `update_document(document_id, title)` - Update document
- `delete_document(document_id)` - Delete document
- `list_documents()` - List documents
- `apply_edit(document_id, position, content, delete_length)` - Apply edit
- `connect_websocket(document_id, client_id, on_edit)` - Connect to WebSocket
- `send_edit(document_id, position, content, delete_length)` - Send edit via WebSocket

### SsoClient

- `list_providers()` - List providers
- `get_provider(provider_id)` - Get provider
- `create_provider(config)` - Create provider
- `update_provider(provider_id, config)` - Update provider
- `delete_provider(provider_id)` - Delete provider
- `get_authorization_url(provider_id, redirect_uri)` - Get auth URL
- `exchange_code(provider_id, code, redirect_uri)` - Exchange code for tokens
- `get_user_info(provider_id, access_token)` - Get user info
- `validate_token(provider_id, token)` - Validate token

## License

MIT