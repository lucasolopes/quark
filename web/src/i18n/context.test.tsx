import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { I18nProvider } from "./context";

describe("I18nProvider — <html lang> sync", () => {
  it("sets document.documentElement.lang to the active locale", () => {
    render(<I18nProvider locale="en">child</I18nProvider>);
    expect(document.documentElement.lang).toBe("en");
  });

  it("uses the pt-BR tag when the locale is Portuguese", () => {
    render(<I18nProvider locale="pt-BR">child</I18nProvider>);
    expect(document.documentElement.lang).toBe("pt-BR");
  });
});
