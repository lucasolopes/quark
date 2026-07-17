import { ArrowLeft } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { StatsView } from "@/components/StatsView";
import { useT } from "@/i18n";

export function LinkStats() {
  const t = useT();
  const { code = "" } = useParams<{ code: string }>();

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("stats.backAria")}
          nativeButton={false}
          render={<Link to="/links" />}
        >
          <ArrowLeft className="size-4" />
        </Button>
      </div>

      <StatsView code={code} />
    </div>
  );
}
