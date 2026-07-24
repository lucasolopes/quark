import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OutOfShellFrame } from "./OutOfShellFrame";

describe("OutOfShellFrame", () => {
  it("renders the title as an h1, the subtitle, the children and the topRight slot", () => {
    render(
      <OutOfShellFrame
        title="Sign in to quark"
        subtitle="Manage your links"
        topRight={<button>Lang switch</button>}
      >
        <p>card content</p>
      </OutOfShellFrame>,
    );

    expect(screen.getByRole("heading", { level: 1, name: "Sign in to quark" })).toBeInTheDocument();
    expect(screen.getByText("Manage your links")).toBeInTheDocument();
    expect(screen.getByText("card content")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Lang switch" })).toBeInTheDocument();
  });

  it("renders the decorative backdrop layers as aria-hidden", () => {
    const { container } = render(
      <OutOfShellFrame title="Title only">
        <p>content</p>
      </OutOfShellFrame>,
    );

    const hiddenLayers = container.querySelectorAll('[aria-hidden="true"]');
    expect(hiddenLayers.length).toBeGreaterThanOrEqual(2);
  });

  it("omits the subtitle and the topRight slot when not provided", () => {
    render(
      <OutOfShellFrame title="Title only">
        <p>content</p>
      </OutOfShellFrame>,
    );

    expect(screen.getByRole("heading", { level: 1, name: "Title only" })).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
