import { useCallback } from 'react';

interface VSCodeAPI {
  postMessage(message: any): void;
  getState(): any;
  setState(state: any): void;
}

// Global VSCode API access
declare function acquireVsCodeApi(): VSCodeAPI;

let vscodeApi: VSCodeAPI | null = null;

function getVsCodeApi(): VSCodeAPI | null {
  try {
    if (!vscodeApi) {
      vscodeApi = acquireVsCodeApi();
    }
    return vscodeApi;
  } catch {
    return null; // Not running inside VSCode
  }
}

export function useVSCode() {
  const postMessage = useCallback((message: any) => {
    const api = getVsCodeApi();
    if (api) {
      api.postMessage(message);
    } else {
      console.log('[CarpAI] Not in VSCode, message:', message);
    }
  }, []);

  const getState = useCallback(() => {
    return getVsCodeApi()?.getState() ?? null;
  }, []);

  const setState = useCallback((state: any) => {
    getVsCodeApi()?.setState(state);
  }, []);

  return { postMessage, getState, setState };
}
