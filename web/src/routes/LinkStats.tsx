import { useParams } from "react-router-dom";
import { PageHeader } from "@/components/PageHeader";
import { StatsView } from "@/components/StatsView";
import { useT } from "@/i18n";

export function LinkStats() {
  const t = useT();
  const { code = "" } = useParams<{ code: string }>();

  return (
    <div className="flex flex-col gap-4 animate-rise max-w-[1000px]">
      <PageHeader
        title={<span className="font-heading">{code}</span>}
        subtitle={t("stats.subtitle", { code })}
        back={{ label: t("stats.backToLinks"), to: "/links" }}
      />

      <StatsView code={code} />
    </div>
  );
}
