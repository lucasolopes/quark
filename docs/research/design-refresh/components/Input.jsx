import React from 'react';
export function Input({ mono = false, style, ...rest }) {
  const base = {
    width: '100%', padding: '11px 14px', background: 'var(--surface-input)',
    border: '1px solid var(--border)', borderRadius: 'var(--radius-md)',
    color: 'var(--text)', fontSize: 14, outline: 'none',
    fontFamily: mono ? 'var(--font-mono)' : 'var(--font-body)', ...style,
  };
  return <input style={base} {...rest} />;
}
