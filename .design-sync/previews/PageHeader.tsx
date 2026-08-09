// PageHeader preview — display title + subtitle + actions, and the back-link shape.
import { Plus } from "lucide-react";
import { Button, PageHeader } from "web";

export function WithActions() {
  return (
    <div className="w-[620px]">
      <PageHeader
        title="Links"
        subtitle="1,982 active links · 48,215 clicks in the last 30 days"
        actions={
          <>
            <Button variant="outline">Export CSV</Button>
            <Button>
              <Plus data-icon="inline-start" /> New link
            </Button>
          </>
        }
      />
    </div>
  );
}

export function WithBackLink() {
  return (
    <div className="w-[620px]">
      <PageHeader
        back={{ label: "← Back to links", to: "/links" }}
        title="spring-sale"
        subtitle="quark.to/spring-sale → example.com/pricing"
      />
    </div>
  );
}
