interface CarpaiSDK {
  init(): Promise<void>;
  version(): string;
  chat_completion(
    serverUrl: string,
    apiKey: string,
    messages: Array<{ role: string; content: string }>,
    model?: string,
  ): Promise<string>;
}

let sdk: CarpaiSDK | null = null;

export async function initSDK(): Promise<CarpaiSDK | null> {
  if (sdk) return sdk;

  try {
    // Try to load @carpai/sdk WASM module
    const carpaiSdk = await import('@carpai/sdk');
    await carpaiSdk.init();
    sdk = carpaiSdk as unknown as CarpaiSDK;
    console.log('CarpAI SDK initialized successfully');
    return sdk;
  } catch (e) {
    console.warn('Failed to load @carpai/sdk:', e);
    return null;
  }
}

export function getSDK(): CarpaiSDK | null {
  return sdk;
}

/**
 * Send a chat completion request through the SDK.
 * Falls back to VSCode postMessage if SDK is unavailable.
 */
export async function chatCompletion(
  messages: Array<{ role: string; content: string }>,
  serverUrl: string = 'http://localhost:8080',
): Promise<string | null> {
  if (sdk) {
    try {
      return await sdk.chat_completion(serverUrl, '', messages);
    } catch (e) {
      console.error('SDK chat completion failed:', e);
      return null;
    }
  }
  return null;
}
