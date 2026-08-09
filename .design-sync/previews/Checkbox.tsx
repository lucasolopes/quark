// Checkbox preview — checked axis + disabled, labeled as in the forms.
import { Checkbox, Label } from "web";

export function States() {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <Checkbox id="c1" defaultChecked />
        <Label htmlFor="c1">Track conversions</Label>
      </div>
      <div className="flex items-center gap-2">
        <Checkbox id="c2" />
        <Label htmlFor="c2">Require password</Label>
      </div>
      <div className="flex items-center gap-2">
        <Checkbox id="c3" disabled defaultChecked />
        <Label htmlFor="c3" className="opacity-50">
          Locked by plan
        </Label>
      </div>
    </div>
  );
}
