import * as React from 'react';
export interface TerminalProps {
  title?: string;
  children?: React.ReactNode;
  style?: React.CSSProperties;
}
export function Terminal(props: TerminalProps): React.ReactElement;
