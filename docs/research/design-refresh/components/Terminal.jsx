import React from 'react';
export function Terminal({ title = 'quark — zsh', children, style }) {
  return (
    <div style={{ border: '1px solid var(--border)', borderRadius: 'var(--radius-lg)', background: 'var(--surface-input)', overflow: 'hidden', boxShadow: 'var(--shadow-modal)', ...style }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 16px', borderBottom: '1px solid var(--border)', background: 'rgba(255,255,255,.02)' }}>
        <span style={{ width: 11, height: 11, borderRadius: '50%', background: '#ff5f57' }} />
        <span style={{ width: 11, height: 11, borderRadius: '50%', background: '#febc2e' }} />
        <span style={{ width: 11, height: 11, borderRadius: '50%', background: '#28c840' }} />
        <span style={{ marginLeft: 8, fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-dim)' }}>{title}</span>
      </div>
      <pre style={{ margin: 0, padding: '22px 20px', fontFamily: 'var(--font-mono)', fontSize: 13.5, lineHeight: 1.85, color: '#C9CEDB', whiteSpace: 'pre-wrap' }}>{children}</pre>
    </div>
  );
}
