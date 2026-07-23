import React from 'react';
export function MeterBar({ label, value, pct, color = 'var(--accent)', track = 'rgba(255,255,255,.05)', style }) {
  return (
    <div style={style}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline', marginBottom: 6 }}>
        <span style={{ fontSize: 15, color: 'var(--text)', fontWeight: 500 }}>{label}</span>
        {value != null && <span style={{ fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-muted)' }}>{value}</span>}
      </div>
      <div style={{ height: 10, background: track, borderRadius: 'var(--radius-sm)', overflow: 'hidden' }}>
        <div style={{ height: '100%', width: Math.max(0, Math.min(100, pct)) + '%', background: color, borderRadius: 'var(--radius-sm)', transition: 'width var(--dur-slow) var(--ease)' }} />
      </div>
    </div>
  );
}
