import { describe, it, expect } from "vitest";
import { tagColor } from "./tag-color";

describe("tagColor", () => {
  it("is stable: the same name always maps to the same swatch", () => {
    expect(tagColor("launch")).toEqual(tagColor("launch"));
    expect(tagColor("launch").dot).toBe(tagColor("launch").dot);
  });

  it("gives two different tags different dot colors", () => {
    // These two names hash into distinct palette slots.
    expect(tagColor("launch").dot).not.toBe(tagColor("summer").dot);
  });

  it("returns dot/text/bg as non-empty CSS strings", () => {
    const c = tagColor("promo");
    expect(c.dot).toMatch(/^#/);
    expect(c.text).toContain("color-mix");
    expect(c.bg).toContain("transparent");
  });
});
