import React from 'react';

const SIZES = {
  sm: { padding: '8px 14px', fontSize: 13.5 },
  md: { padding: '11px 20px', fontSize: 14 },
  lg: { padding: '14px 24px', fontSize: 15 },
};

export function Button({ variant = 'primary', size = 'md', disabled, children, style, ...rest }) {
  const s = SIZES[size] || SIZES.md;
  const base = {
    display: 'inline-flex', alignItems: 'center', gap: 8, justifyContent: 'center',
    fontFamily: 'var(--font-body)', fontWeight: variant === 'primary' ? 700 : 600,
    borderRadius: 'var(--radius-md)', cursor: disabled ? 'not-allowed' : 'pointer',
    padding: s.padding, fontSize: s.fontSize, border: '1px solid transparent',
    transition: 'background var(--dur-fast) var(--ease), border-color var(--dur-fast) var(--ease), transform var(--dur-fast) var(--ease)',
    opacity: disabled ? 0.5 : 1, whiteSpace: 'nowrap', ...style,
  };
  const variants = {
    primary: { background: 'var(--accent)', color: 'var(--on-accent)' },
    secondary: { background: 'transparent', color: 'var(--text)', borderColor: 'var(--border-strong)' },
    ghost: { background: 'transparent', color: 'var(--text-muted)' },
    danger: { background: 'var(--danger)', color: '#fff' },
  };
  return <button disabled={disabled} style={{ ...base, ...variants[variant] }} {...rest}>{children}</button>;
}
