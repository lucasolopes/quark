// Textarea preview — filled and disabled.
import { Label, Textarea } from "web";

export function States() {
  return (
    <div className="flex w-96 flex-col gap-4">
      <div className="flex flex-col gap-2">
        <Label htmlFor="notes">Internal notes</Label>
        <Textarea
          id="notes"
          defaultValue="Campaign link for the spring launch email. Rotates to the EU landing page for European visitors."
        />
      </div>
      <Textarea disabled placeholder="Disabled — upgrade to edit" />
    </div>
  );
}
