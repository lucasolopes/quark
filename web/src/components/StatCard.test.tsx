import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { StatCard } from "./StatCard";

describe("StatCard", () => {
  it("renders value and label", () => {
    render(<StatCard value="1,234" label="Links" />);
    expect(screen.getByText("1,234")).toBeInTheDocument();
    expect(screen.getByText("Links")).toBeInTheDocument();
  });

  it("renders value with text-brand-ink when accent is true", () => {
    render(<StatCard value="1,234" label="Links" accent={true} />);
    const valueDiv = screen.getByText("1,234");
    expect(valueDiv).toHaveClass("text-brand-ink");
  });

  it("renders value with text-strong when accent is false or undefined", () => {
    render(<StatCard value="1,234" label="Links" />);
    const valueDiv = screen.getByText("1,234");
    expect(valueDiv).toHaveClass("text-strong");
  });
});
