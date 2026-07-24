import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Terminal } from "./Terminal";

describe("Terminal", () => {
  it("renders children inside a <pre> tag", () => {
    const { container } = render(<Terminal>npm start</Terminal>);
    const preTag = container.querySelector("pre");
    expect(preTag).toBeInTheDocument();
    expect(preTag).toHaveTextContent("npm start");
  });

  it("renders default title 'quark — zsh'", () => {
    render(<Terminal>content</Terminal>);
    expect(screen.getByText("quark — zsh")).toBeInTheDocument();
  });

  it("renders three traffic lights, hidden from assistive tech (decorative)", () => {
    render(<Terminal>content</Terminal>);
    const trafficLights = screen.getAllByTestId("traffic-light");
    expect(trafficLights).toHaveLength(3);
    for (const light of trafficLights) {
      expect(light).toHaveAttribute("aria-hidden", "true");
    }
  });

  it("renders custom title", () => {
    render(<Terminal title="custom title">content</Terminal>);
    expect(screen.getByText("custom title")).toBeInTheDocument();
  });
});
