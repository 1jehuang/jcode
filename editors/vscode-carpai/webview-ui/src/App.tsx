import React, { useState, useEffect, useCallback } from 'react';
import { ChatView } from './components/ChatView';
import { InputBar } from './components/InputBar';

interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: number;
}

function App() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [serverConnected, setServerConnected] = useState(false);

  useEffect(() => {
    // Initialize @carpai/sdk
    async function init() {
      try {
        // @ts-ignore - carpai SDK will be available as WASM
        if (window.carpaiSdk) {
          await window.carpaiSdk.init();
          setServerConnected(true);
        }
      } catch (e) {
        console.warn('CarpAI SDK not available, falling back to VSCode API');
      }
    }
    init();
  }, []);

  const handleSend = useCallback((text: string) => {
    const userMsg: Message = {
      id: `user-${Date.now()}`,
      role: 'user',
      content: text,
      timestamp: Date.now(),
    };
    setMessages(prev => [...prev, userMsg]);
    setIsLoading(true);

    // Send via VSCode postMessage API
    // @ts-ignore - VSCode API
    const vscode = acquireVsCodeApi();
    vscode.postMessage({
      type: 'chat',
      message: text,
    });
  }, []);

  // Listen for responses from VSCode extension
  useEffect(() => {
    function handleResponse(event: MessageEvent) {
      const msg = event.data;
      if (msg.type === 'chatResponse') {
        const assistantMsg: Message = {
          id: `assistant-${Date.now()}`,
          role: 'assistant',
          content: msg.response,
          timestamp: Date.now(),
        };
        setMessages(prev => [...prev, assistantMsg]);
        setIsLoading(false);
      } else if (msg.type === 'error') {
        const errorMsg: Message = {
          id: `error-${Date.now()}`,
          role: 'system',
          content: `Error: ${msg.message}`,
          timestamp: Date.now(),
        };
        setMessages(prev => [...prev, errorMsg]);
        setIsLoading(false);
      }
    }

    window.addEventListener('message', handleResponse);
    return () => window.removeEventListener('message', handleResponse);
  }, []);

  return (
    <>
      <ChatView messages={messages} />
      <InputBar onSend={handleSend} disabled={isLoading} />
    </>
  );
}

export default App;
