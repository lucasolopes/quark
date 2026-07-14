import { describe, it, expect, beforeEach } from "vitest";
import { applyUtm, loadUtmTemplates, saveUtmTemplate, deleteUtmTemplate } from "./utm";

describe("applyUtm", () => {
  it("appends utm params to a bare URL", () => {
    const result = applyUtm("https://example.com/page", { source: "newsletter", medium: "email" });
    const parsed = new URL(result);
    expect(parsed.searchParams.get("utm_source")).toBe("newsletter");
    expect(parsed.searchParams.get("utm_medium")).toBe("email");
  });

  it("preserves an existing query string and adds utm params alongside it", () => {
    const result = applyUtm("https://example.com/page?ref=123", { campaign: "summer" });
    const parsed = new URL(result);
    expect(parsed.searchParams.get("ref")).toBe("123");
    expect(parsed.searchParams.get("utm_campaign")).toBe("summer");
  });

  it("overwrites a utm param that is already present in the URL", () => {
    const result = applyUtm("https://example.com/page?utm_source=old", { source: "new" });
    const parsed = new URL(result);
    expect(parsed.searchParams.getAll("utm_source")).toEqual(["new"]);
  });

  it("is a no-op when no params are provided (or all are empty)", () => {
    expect(applyUtm("https://example.com/page", {})).toBe("https://example.com/page");
    expect(applyUtm("https://example.com/page", { source: "", medium: "   " })).toBe(
      "https://example.com/page",
    );
  });

  it("returns the original url unchanged when it fails to parse", () => {
    expect(applyUtm("not-a-url", { source: "x" })).toBe("not-a-url");
  });
});

describe("UTM templates (localStorage)", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("round-trips a saved template", () => {
    saveUtmTemplate("Spring launch", { source: "twitter", medium: "social" });
    const templates = loadUtmTemplates();
    expect(templates["Spring launch"]).toEqual({ source: "twitter", medium: "social" });
  });

  it("overwrites a template saved under the same name", () => {
    saveUtmTemplate("promo", { source: "a" });
    saveUtmTemplate("promo", { source: "b" });
    expect(loadUtmTemplates()).toEqual({ promo: { source: "b" } });
  });

  it("deletes a template", () => {
    saveUtmTemplate("temp", { source: "x" });
    deleteUtmTemplate("temp");
    expect(loadUtmTemplates()).toEqual({});
  });

  it("tolerates a missing store", () => {
    expect(loadUtmTemplates()).toEqual({});
  });

  it("tolerates a corrupted store", () => {
    localStorage.setItem("quark.utmTemplates", "{not json");
    expect(loadUtmTemplates()).toEqual({});
  });
});
