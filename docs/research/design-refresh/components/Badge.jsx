import React from 'react';
export function Badge({ variant = 'mono', children, style, ...rest }) {
  const base = {
    display: 'inline-flex', alignItems: 'center', fontFamily: 'var(--font-mono)',
    fontSize: 'var(--fs-chip)', lineHeight: 1.4, padding: '2px 6px',
    borderRadius: 'var(--radius-xs)', letterSpacing: '0.02em', ...style,
  };
  const variants = {
    mono: { color: 'var(--text-muted)', border: '1px solid var(--border-strong)' },
    accent: { color: 'var(--accent)', border: '1px solid var(--accent-line)', background: 'var(--accent-wash)' },
    solid: { color: 'var(--on-accent)', background: 'var(--accent)' },
  };
  return <span style={{ ...base, ...variants[variant] }} {...rest}>{children}</span>;
}
