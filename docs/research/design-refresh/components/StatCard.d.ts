import * as React from 'react';
export interface StatCardProps extends React.HTMLAttributes<HTMLDivElement> {
  value: React.ReactNode;
  label: React.ReactNode;
  /** Wrap in a bordered card (capacity band) vs bare (hero stats). */
  boxed?: boolean;
}
export function StatCard(props: StatCardProps): React.ReactElement;
