import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MeterBar } from "./MeterBar";

describe("MeterBar", () => {
  it("clamps pct between 0 and 100 with scaleX transform", () => {
    const { container: container50 } = render(<MeterBar label="USA" pct={50} />);
    const bar50 = container50.querySelector("div[style*='transform']") as HTMLElement;
    expect(bar50).toHaveStyle("transform: scaleX(0.5)");

    const { container: container150 } = render(<MeterBar label="USA" pct={150} />);
    const bar150 = container150.querySelector("div[style*='transform']") as HTMLElement;
    expect(bar150).toHaveStyle("transform: scaleX(1)");
  });

  it("renders optional value", () => {
    render(<MeterBar label="USA" value="42%" pct={42} />);
    expect(screen.getByText("42%")).toBeInTheDocument();
  });

  it("renders label", () => {
    render(<MeterBar label="USA" pct={42} />);
    expect(screen.getByText("USA")).toBeInTheDocument();
  });
});
