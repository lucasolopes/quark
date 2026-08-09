// CollapsibleSection preview — open and collapsed hairline sections.
import { CollapsibleSection, Input, Label } from "web";

export function OpenAndClosed() {
  return (
    <div className="flex w-96 flex-col gap-3">
      <CollapsibleSection title="UTM parameters" defaultOpen>
        <div className="flex flex-col gap-2">
          <Label htmlFor="utm-src">utm_source</Label>
          <Input id="utm-src" defaultValue="newsletter" />
        </div>
      </CollapsibleSection>
      <CollapsibleSection title="Password protection">
        <div />
      </CollapsibleSection>
      <CollapsibleSection title="Scheduling">
        <div />
      </CollapsibleSection>
    </div>
  );
}
