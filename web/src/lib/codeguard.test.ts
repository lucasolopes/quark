import { describe, it, expect } from "vitest";
import { isNumericCode, isHttpUrl } from "./codeguard";

describe("isNumericCode", () => {
  it("accepts a common (non-numeric) alias", () => {
    expect(isNumericCode("promo23")).toBe(false);
  });

  it("rejects a length different from 7", () => {
    expect(isNumericCode("abc")).toBe(false);
    expect(isNumericCode("abcdefgh")).toBe(false);
    expect(isNumericCode("")).toBe(false);
  });

  it("rejects a character outside the base62 alphabet", () => {
    expect(isNumericCode("abc-123")).toBe(false);
    expect(isNumericCode("café123")).toBe(false);
  });

  it("detects the smallest numeric code (zeros)", () => {
    expect(isNumericCode("0000000")).toBe(true);
  });

  it("detects the largest numeric code (2^40 - 1)", () => {
    expect(isNumericCode("JMAIjoV")).toBe(true);
  });

  it("accepts a valid 7-char string whose value exceeds 2^40 - 1", () => {
    expect(isNumericCode("JMAIjoW")).toBe(false);
  });

  it("is case-sensitive (0-9A-Za-z alphabet)", () => {
    expect(isNumericCode("Aaaaaaa")).toBe(true);
    expect(isNumericCode("aAAAAAA")).toBe(false);
  });
});

describe("isHttpUrl", () => {
  it("accepts http and https", () => {
    expect(isHttpUrl("http://example.com")).toBe(true);
    expect(isHttpUrl("https://example.com/a/b?c=1")).toBe(true);
    expect(isHttpUrl("  https://example.com  ")).toBe(true);
  });

  it("rejects other schemes and text without a scheme", () => {
    expect(isHttpUrl("ftp://example.com")).toBe(false);
    expect(isHttpUrl("javascript:alert(1)")).toBe(false);
    expect(isHttpUrl("example.com")).toBe(false);
    expect(isHttpUrl("")).toBe(false);
  });

  it("is case-sensitive, like the backend (starts_with)", () => {
    expect(isHttpUrl("HTTP://example.com")).toBe(false);
    expect(isHttpUrl("HTTPS://example.com")).toBe(false);
  });

  it("doesn't validate the URL itself, only the prefix (same behavior as the backend)", () => {
    expect(isHttpUrl("http://")).toBe(true);
    expect(isHttpUrl("https://not a valid url")).toBe(true);
  });
});
