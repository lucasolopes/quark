// MeterBar preview — distribution bars (country/device breakdown) in all tones.
import { MeterBar } from "web";

export function Breakdown() {
  return (
    <div className="flex w-80 flex-col gap-4">
      <MeterBar label="Brazil" value="18,204" pct={62} tone="accent" />
      <MeterBar label="United States" value="7,911" pct={27} tone="accent" />
      <MeterBar label="Germany" value="3,110" pct={11} tone="accent" />
    </div>
  );
}

export function Tones() {
  return (
    <div className="flex w-80 flex-col gap-4">
      <MeterBar label="Direct" value="54%" pct={54} tone="accent" />
      <MeterBar label="Mobile" value="31%" pct={31} tone="cyan" />
      <MeterBar label="Desktop" value="15%" pct={15} tone="violet" />
    </div>
  );
}
