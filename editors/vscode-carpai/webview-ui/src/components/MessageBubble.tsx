import React from 'react';
import ReactMarkdown from 'react-markdown';

interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: number;
}

interface Props {
  message: Message;
}

const roleStyles: Record<string, React.CSSProperties> = {
  user: {
    background: 'var(--vscode-button-background, #007acc)',
    color: 'var(--vscode-button-foreground, white)',
    alignSelf: 'flex-end',
    maxWidth: '80%',
  },
  assistant: {
    background: 'var(--vscode-editor-inactiveSelectionBackground, #2d2d30)',
    border: '1px solid var(--vscode-widget-border, #3c3c3c)',
    alignSelf: 'flex-start',
    maxWidth: '85%',
  },
  system: {
    background: 'transparent',
    color: 'var(--vscode-errorForeground, #f48771)',
    alignSelf: 'center',
    fontSize: '12px',
    maxWidth: '90%',
  },
};

export function MessageBubble({ message }: Props) {
  return (
    <div
      style={{
        padding: '12px 16px',
        borderRadius: '8px',
        ...roleStyles[message.role],
      }}
    >
      <div style={{ fontSize: '11px', fontWeight: 600, marginBottom: '4px', opacity: 0.7 }}>
        {message.role === 'user' ? 'You' : message.role === 'assistant' ? 'CarpAI' : 'System'}
      </div>
      <div style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word', lineHeight: 1.5 }}>
        {message.role === 'assistant' ? (
          <ReactMarkdown>{message.content}</ReactMarkdown>
        ) : (
          message.content
        )}
      </div>
    </div>
  );
}
