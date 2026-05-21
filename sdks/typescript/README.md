# JCode TypeScript SDK

TypeScript SDK for the JCode AI Code Assistant.

## Installation

```bash
npm install @jcode/sdk
```

## Quick Start

```typescript
import { JCodeClient } from '@jcode/sdk';

// Initialize client
const client = new JCodeClient('http://localhost:8080');

// Get code completions
const completions = await client.complete(
  'function hello() {',
  'javascript',
  0,
  17
);

console.log(completions);
```

## Features

### Code Completion

```typescript
import { CompletionClient } from '@jcode/sdk';

const client = new CompletionClient('http://localhost:8080');

// Get completions
const candidates = await client.complete(
  'const fibonacci = (n) => {',
  'javascript',
  0,
  24
);

// Stream completions
await client.completeStream(
  'class MyClass {',
  'typescript',
  0,
  14,
  undefined,
  (candidate) => {
    console.log(candidate.label);
  }
);
```

### CRDT Collaboration

```typescript
import { CrdtClient } from '@jcode/sdk';

const client = new CrdtClient('http://localhost:8080');

// Create document
const doc = await client.createDocument('My Document', 'Initial content');

// Apply edit
const operation = await client.applyEdit(
  doc.document_id,
  15,
  ' edited'
);

// Get document
const updatedDoc = await client.getDocument(doc.document_id);
console.log(updatedDoc.content); // "Initial content edited"
```

### SSO Authentication

```typescript
import { SsoClient } from '@jcode/sdk';

const client = new SsoClient('http://localhost:8080');

// List providers
const providers = await client.listProviders();

// Get authorization URL
const authUrl = await client.getAuthorizationUrl(
  'my-provider',
  'http://localhost:3000/callback'
);

// Exchange code for token
const tokens = await client.exchangeCode(
  'my-provider',
  'authorization-code',
  'http://localhost:3000/callback'
);

// Get user info
const userInfo = await client.getUserInfo(
  'my-provider',
  tokens.access_token
);
```

## API Reference

### JCodeClient

- `healthCheck()` - Check server health
- `getVersion()` - Get server version
- `complete(content, language, cursorLine, cursorColumn, filePath)` - Get completions
- `close()` - Close connections

### CompletionClient

- `complete(content, language, cursorLine, cursorColumn, filePath)` - Get completions
- `completeStream(content, language, cursorLine, cursorColumn, filePath, onCandidate)` - Stream completions
- `getContext(content, cursorLine, cursorColumn)` - Get completion context
- `getStats()` - Get statistics

### CrdtClient

- `createDocument(title, content)` - Create document
- `getDocument(documentId)` - Get document
- `updateDocument(documentId, title)` - Update document
- `deleteDocument(documentId)` - Delete document
- `listDocuments()` - List documents
- `applyEdit(documentId, position, content, deleteLength)` - Apply edit
- `connectWebSocket(documentId, clientId, onEdit)` - Connect to WebSocket
- `sendEdit(documentId, position, content, deleteLength)` - Send edit via WebSocket
- `closeWebSocket()` - Close WebSocket connection

### SsoClient

- `listProviders()` - List providers
- `getProvider(providerId)` - Get provider
- `createProvider(config)` - Create provider
- `updateProvider(providerId, config)` - Update provider
- `deleteProvider(providerId)` - Delete provider
- `getAuthorizationUrl(providerId, redirectUri)` - Get auth URL
- `exchangeCode(providerId, code, redirectUri)` - Exchange code for tokens
- `getUserInfo(providerId, accessToken)` - Get user info
- `validateToken(providerId, token)` - Validate token

## License

MIT