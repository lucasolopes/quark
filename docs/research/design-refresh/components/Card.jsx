import React from 'react';
export function Card({ padding = 26, hover = false, children, style, ...rest }) {
  const base = {
    background: 'var(--surface-card)', border: '1px solid var(--border)',
    borderRadius: 'var(--radius-xl)', padding, transition: 'border-color var(--dur) var(--ease), transform var(--dur) var(--ease)', ...style,
  };
  return <div className={hover ? 'ds-card-hover' : undefined} style={base} {...rest}>{children}</div>;
}
