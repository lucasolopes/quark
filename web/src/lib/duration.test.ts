import { describe, expect, it } from "vitest";
import { DEFAULT_DURATION_UNIT, durationToSeconds } from "./duration";

describe("durationToSeconds", () => {
  it("converts each unit to seconds", () => {
    expect(durationToSeconds("1", "minutes")).toBe(60);
    expect(durationToSeconds("2", "hours")).toBe(7200);
    expect(durationToSeconds("3", "days")).toBe(259200);
    expect(durationToSeconds("1", "weeks")).toBe(604800);
    expect(durationToSeconds("1", "months")).toBe(2592000);
  });

  it("returns null for a blank value", () => {
    expect(durationToSeconds("", "days")).toBeNull();
    expect(durationToSeconds("   ", "days")).toBeNull();
  });

  it("returns null for non-integer or non-positive values", () => {
    expect(durationToSeconds("0", "days")).toBeNull();
    expect(durationToSeconds("-1", "hours")).toBeNull();
    expect(durationToSeconds("1.5", "days")).toBeNull();
    expect(durationToSeconds("abc", "days")).toBeNull();
  });

  it("defaults to days", () => {
    expect(DEFAULT_DURATION_UNIT).toBe("days");
    expect(durationToSeconds("1", DEFAULT_DURATION_UNIT)).toBe(86400);
  });
});
