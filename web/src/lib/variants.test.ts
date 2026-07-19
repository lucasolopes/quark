import { describe, expect, it } from "vitest";
import { distributeEvenly, normalizeToPercent, variantsPercentTotal } from "./variants";

describe("distributeEvenly", () => {
  it("returns an empty array for zero or negative counts", () => {
    expect(distributeEvenly(0)).toEqual([]);
    expect(distributeEvenly(-1)).toEqual([]);
  });

  it("splits evenly when the count divides 100", () => {
    expect(distributeEvenly(2)).toEqual([50, 50]);
    expect(distributeEvenly(4)).toEqual([25, 25, 25, 25]);
  });

  it("hands the remainder to the first variants so the sum is exactly 100", () => {
    expect(distributeEvenly(3)).toEqual([34, 33, 33]);
    expect(distributeEvenly(7)).toEqual([15, 15, 14, 14, 14, 14, 14]);
    expect(distributeEvenly(3).reduce((a, b) => a + b, 0)).toBe(100);
    expect(distributeEvenly(7).reduce((a, b) => a + b, 0)).toBe(100);
  });
});

describe("normalizeToPercent", () => {
  it("returns an empty array for no weights", () => {
    expect(normalizeToPercent([])).toEqual([]);
  });

  it("presents legacy equal weights as an even percentage split", () => {
    expect(normalizeToPercent([1, 1])).toEqual([50, 50]);
    expect(normalizeToPercent([1, 1, 1])).toEqual([34, 33, 33]);
  });

  it("preserves proportions", () => {
    expect(normalizeToPercent([3, 1])).toEqual([75, 25]);
    expect(normalizeToPercent([1, 2, 1])).toEqual([25, 50, 25]);
  });

  it("always sums to exactly 100 using largest-remainder rounding", () => {
    const out = normalizeToPercent([1, 1, 1]);
    expect(out.reduce((a, b) => a + b, 0)).toBe(100);
    const out2 = normalizeToPercent([1, 1, 1, 1, 1, 1, 1]);
    expect(out2.reduce((a, b) => a + b, 0)).toBe(100);
  });

  it("falls back to an even split when the total is zero", () => {
    expect(normalizeToPercent([0, 0])).toEqual([50, 50]);
  });
});

describe("variantsPercentTotal", () => {
  it("sums the numeric percentage strings", () => {
    expect(variantsPercentTotal(["50", "50"])).toBe(100);
    expect(variantsPercentTotal(["20", "30"])).toBe(50);
  });

  it("treats blank or non-numeric entries as zero", () => {
    expect(variantsPercentTotal(["50", "", "  "])).toBe(50);
    expect(variantsPercentTotal(["abc", "40"])).toBe(40);
  });
});
