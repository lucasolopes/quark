// TtlChips preview — expiration presets with different selections.
import { TtlChips } from "web";

const noop = () => {};

export function DaySelected() {
  return <TtlChips value="24" unit="hours" onChange={noop} />;
}

export function NeverSelected() {
  return <TtlChips value="" unit="minutes" onChange={noop} />;
}

export function Disabled() {
  return <TtlChips value="7" unit="days" onChange={noop} disabled />;
}
