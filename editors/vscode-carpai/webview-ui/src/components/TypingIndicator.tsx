import React from 'react';

export function TypingIndicator() {
  return (
    <div style={{ padding: '12px 16px', display: 'flex', gap: '4px', alignItems: 'center' }}>
      <span style={{
        width: '8px', height: '8px', borderRadius: '50%',
        background: '#007acc', animation: 'bounce 1.4s infinite ease-in-out',
        display: 'inline-block',
      }} />
      <span style={{
        width: '8px', height: '8px', borderRadius: '50%',
        background: '#007acc', animation: 'bounce 1.4s infinite ease-in-out 0.2s',
        display: 'inline-block',
      }} />
      <span style={{
        width: '8px', height: '8px', borderRadius: '50%',
        background: '#007acc', animation: 'bounce 1.4s infinite ease-in-out 0.4s',
        display: 'inline-block',
      }} />
    </div>
  );
}
