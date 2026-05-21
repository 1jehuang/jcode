/**
 * JCode TypeScript SDK
 * 
 * This SDK provides access to the JCode AI Code Assistant API, enabling:
 * - Code completion
 * - CRDT-based collaborative editing
 * - SSO authentication
 * - Real-time collaboration via WebSocket
 * 
 * Example:
 * ```typescript
 * import { JCodeClient } from '@jcode/sdk';
 * 
 * const client = new JCodeClient('http://localhost:8080');
 * const completions = await client.complete('function hello() {', 'javascript');
 * console.log(completions);
 * ```
 */

export { JCodeClient } from './client';
export { CompletionClient } from './completion';
export { CrdtClient } from './crdt';
export { SsoClient } from './sso';

export * from './models';

export const VERSION = '0.12.0';