// Input preview — states used across the link forms.
import { Input, Label } from "web";

export function States() {
  return (
    <div className="flex w-80 flex-col gap-4">
      <Input placeholder="https://example.com/very/long/destination" />
      <Input defaultValue="launch-page" />
      <Input disabled defaultValue="read-only-code" />
      <Input aria-invalid defaultValue="not a url" />
    </div>
  );
}

export function WithLabel() {
  return (
    <div className="flex w-80 flex-col gap-2">
      <Label htmlFor="dest">Destination URL</Label>
      <Input id="dest" placeholder="https://example.com/pricing" />
    </div>
  );
}
