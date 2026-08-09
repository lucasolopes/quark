// Label preview — composed with its form controls (its only real context).
import { Checkbox, Input, Label } from "web";

export function WithInput() {
  return (
    <div className="flex w-80 flex-col gap-2">
      <Label htmlFor="code">Custom code</Label>
      <Input id="code" placeholder="spring-sale" />
    </div>
  );
}

export function WithCheckbox() {
  return (
    <div className="flex items-center gap-2">
      <Checkbox id="qr" defaultChecked />
      <Label htmlFor="qr">Generate QR code</Label>
    </div>
  );
}
