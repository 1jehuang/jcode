import React, { useState, useRef, useEffect } from 'react';

interface Props {
  onSend: (text: string) => void;
  disabled?: boolean;
}

export function InputBar({ onSend, disabled }: Props) {
  const [input, setInput] = useState('');
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!disabled && inputRef.current) {
      inputRef.current.focus();
    }
  }, [disabled]);

  function handleSubmit() {
    const text = input.trim();
    if (!text || disabled) return;
    onSend(text);
    setInput('');
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  }

  return (
    <div
      style={{
        display: 'flex',
        gap: '8px',
        padding: '12px 16px',
        borderTop: '1px solid var(--vscode-widget-border, #3c3c3c)',
        background: 'var(--vscode-input-background, #2d2d30)',
      }}
    >
      <textarea
        ref={inputRef}
        value={input}
        onChange={e => setInput(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Type a message... (Shift+Enter for new line)"
        rows={1}
        disabled={disabled}
        style={{
          flex: 1,
          padding: '8px 12px',
          border: '1px solid var(--vscode-widget-border, #3c3c3c)',
          borderRadius: '6px',
          background: 'var(--vscode-input-background, #1e1e1e)',
          color: 'var(--vscode-input-foreground, #d4d4d4)',
          fontFamily: 'inherit',
          fontSize: '14px',
          outline: 'none',
          resize: 'none',
          minHeight: '36px',
          maxHeight: '120px',
        }}
      />
      <button
        onClick={handleSubmit}
        disabled={disabled || !input.trim()}
        style={{
          padding: '8px 20px',
          background: disabled
            ? 'var(--vscode-button-secondaryBackground, #3a3d41)'
            : 'var(--vscode-button-background, #007acc)',
          color: disabled
            ? 'var(--vscode-button-secondaryForeground, #888)'
            : 'var(--vscode-button-foreground, white)',
          border: 'none',
          borderRadius: '6px',
          cursor: disabled ? 'not-allowed' : 'pointer',
          fontSize: '14px',
          fontWeight: 500,
          alignSelf: 'flex-end',
        }}
      >
        Send
      </button>
    </div>
  );
}
