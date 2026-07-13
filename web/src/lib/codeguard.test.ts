import { describe, it, expect } from "vitest";
import { isNumericCode, isHttpUrl } from "./codeguard";

describe("isNumericCode", () => {
  it("aceita um alias comum (não numérico)", () => {
    expect(isNumericCode("promo23")).toBe(false);
  });

  it("rejeita comprimento diferente de 7", () => {
    expect(isNumericCode("abc")).toBe(false);
    expect(isNumericCode("abcdefgh")).toBe(false);
    expect(isNumericCode("")).toBe(false);
  });

  it("rejeita caractere fora do alfabeto base62", () => {
    expect(isNumericCode("abc-123")).toBe(false);
    expect(isNumericCode("café123")).toBe(false);
  });

  it("detecta o menor código numérico (zeros)", () => {
    expect(isNumericCode("0000000")).toBe(true);
  });

  it("detecta o maior código numérico (2^40 - 1)", () => {
    expect(isNumericCode("JMAIjoV")).toBe(true);
  });

  it("aceita string de 7 chars válida cujo valor excede 2^40 - 1", () => {
    // Um a mais que o maior código numérico: mesmo alfabeto, mesmo
    // comprimento, mas fora do espaço reservado — pode ser alias.
    expect(isNumericCode("JMAIjoW")).toBe(false);
  });

  it("é sensível a maiúsculas/minúsculas (alfabeto 0-9A-Za-z)", () => {
    expect(isNumericCode("Aaaaaaa")).toBe(true);
    expect(isNumericCode("aAAAAAA")).toBe(false);
  });
});

describe("isHttpUrl", () => {
  it("aceita http e https", () => {
    expect(isHttpUrl("http://exemplo.com")).toBe(true);
    expect(isHttpUrl("https://exemplo.com/a/b?c=1")).toBe(true);
    expect(isHttpUrl("  https://exemplo.com  ")).toBe(true);
  });

  it("rejeita outros esquemas e texto sem esquema", () => {
    expect(isHttpUrl("ftp://exemplo.com")).toBe(false);
    expect(isHttpUrl("javascript:alert(1)")).toBe(false);
    expect(isHttpUrl("exemplo.com")).toBe(false);
    expect(isHttpUrl("")).toBe(false);
  });
});
