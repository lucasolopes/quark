import React from 'react';
export function StatCard({ value, label, boxed = false, style, ...rest }) {
  const wrap = boxed
    ? { background: 'var(--surface-card)', border: '1px solid var(--border)', borderRadius: 'var(--radius-lg)', padding: 22 }
    : {};
  return (
    <div style={{ ...wrap, ...style }} {...rest}>
      <div style={{ fontFamily: 'var(--font-display)', fontWeight: 700, fontSize: 'var(--fs-stat)', letterSpacing: 'var(--tr-stat)', color: 'var(--accent)' }}>{value}</div>
      <div style={{ fontSize: 13, lineHeight: 1.5, color: 'var(--text-muted)', marginTop: 6, maxWidth: 160 }}>{label}</div>
    </div>
  );
}
