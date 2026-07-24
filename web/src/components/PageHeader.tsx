import type { ReactNode } from "react";
import { Link } from "react-router-dom";

interface PageHeaderProps {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  /** Link de volta acima do título (padrão da tela de stats do mock). */
  back?: { label: string; to: string };
}

/** Cabeçalho de página do Quark DS: título display + subtítulo muted + ações à direita. */
export function PageHeader({ title, subtitle, actions, back }: PageHeaderProps) {
  return (
    <div className="mb-5">
      {back && (
        <Link to={back.to} className="mb-3 inline-block text-subtitle text-muted-foreground hover:text-foreground">
          {back.label}
        </Link>
      )}
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="min-w-0">
          <h1 className="font-heading text-page-title font-bold tracking-display text-strong">{title}</h1>
          {subtitle && <div className="mt-1 text-subtitle text-muted-foreground">{subtitle}</div>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
    </div>
  );
}
