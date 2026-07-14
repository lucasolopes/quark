import { describe, it, expect, beforeEach } from "vitest";
import { getToken, setToken, clearToken, hasToken } from "./auth";

describe("auth token store", () => {
  beforeEach(() => localStorage.clear());
  it("set/get/has/clear", () => {
    expect(hasToken()).toBe(false);
    setToken("secret");
    expect(getToken()).toBe("secret");
    expect(hasToken()).toBe(true);
    clearToken();
    expect(getToken()).toBeNull();
  });
});
