import * as React from 'react';
export interface MeterBarProps {
  label: React.ReactNode;
  value?: React.ReactNode;
  /** 0–100 fill width. */
  pct: number;
  /** Fill color — accent for the hero row, cyan/violet for segments. */
  color?: string;
  track?: string;
  style?: React.CSSProperties;
}
export function MeterBar(props: MeterBarProps): React.ReactElement;
