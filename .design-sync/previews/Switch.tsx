// Switch preview — on/off/disabled with labels.
import { Label, Switch } from "web";

export function States() {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <Switch id="s1" defaultChecked />
        <Label htmlFor="s1">Link enabled</Label>
      </div>
      <div className="flex items-center gap-2">
        <Switch id="s2" />
        <Label htmlFor="s2">Forward query params</Label>
      </div>
      <div className="flex items-center gap-2">
        <Switch id="s3" disabled />
        <Label htmlFor="s3" className="opacity-50">
          SSO required (admin)
        </Label>
      </div>
    </div>
  );
}
