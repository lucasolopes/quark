import { describe, expect, it } from "vitest";
import { resolveShortHost } from "./short-url";

describe("resolveShortHost", () => {
  it("primaryHost takes precedence over all other fields", () => {
    expect(
      resolveShortHost({
        primaryHost: "go.acme.com",
        slug: "acme",
        suffix: ".q.rk",
        publicHost: "short.example.com",
      }),
    ).toBe("go.acme.com");
  });

  it("slug + suffix are used when primaryHost is absent", () => {
    expect(
      resolveShortHost({
        primaryHost: undefined,
        slug: "acme",
        suffix: "q.rk",
        publicHost: "short.example.com",
      }),
    ).toBe("acme.q.rk");
  });

  it("publicHost is used when primaryHost and slug+suffix are absent", () => {
    expect(
      resolveShortHost({
        primaryHost: undefined,
        slug: undefined,
        suffix: ".q.rk",
        publicHost: "short.example.com",
      }),
    ).toBe("short.example.com");
  });

  it("falls back to PUBLIC_BASE_HOST when no other field is present", () => {
    const result = resolveShortHost({
      primaryHost: undefined,
      slug: undefined,
      suffix: undefined,
      publicHost: undefined,
    });
    // The fallback is PUBLIC_BASE_HOST, which is the origin with protocol stripped
    expect(typeof result).toBe("string");
    expect(result.length).toBeGreaterThan(0);
  });
});
