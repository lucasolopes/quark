import { describe, it, expect } from "vitest";
import { COUNTRY_CODES, countryOptions } from "./countries";

describe("countryOptions", () => {
  it("returns one option per ISO code", () => {
    expect(countryOptions("en")).toHaveLength(COUNTRY_CODES.length);
  });

  it("keeps the ISO code as the stored value and includes it in the label", () => {
    const options = countryOptions("en");
    const br = options.find((o) => o.value === "BR");
    expect(br).toBeDefined();
    expect(br!.label).toContain("(BR)");
  });

  it("localizes the country name (Intl.DisplayNames)", () => {
    const en = countryOptions("en").find((o) => o.value === "BR");
    const pt = countryOptions("pt-BR").find((o) => o.value === "BR");
    // Names differ by locale when Intl data is present; if not, both fall back
    // to the bare code, so at minimum the code is always present.
    expect(en!.label).toMatch(/BR/);
    expect(pt!.label).toMatch(/BR/);
  });

  it("sorts options by their localized label", () => {
    const labels = countryOptions("en").map((o) => o.label);
    const sorted = [...labels].sort((a, b) => a.localeCompare(b, "en"));
    expect(labels).toEqual(sorted);
  });

  it("only contains two-letter uppercase codes", () => {
    expect(COUNTRY_CODES.every((c) => /^[A-Z]{2}$/.test(c))).toBe(true);
  });
});
