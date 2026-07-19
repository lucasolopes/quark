import { describe, it, expect } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Combobox, type ComboboxOption } from "./Combobox";
import { withProviders } from "@/test-utils";

function Harness({
  multiple,
  createLabel,
  options,
  initial = [],
}: {
  multiple?: boolean;
  createLabel?: string;
  options: ComboboxOption[];
  initial?: string[];
}) {
  const [value, setValue] = useState<string[]>(initial);
  return (
    <>
      <Combobox
        options={options}
        value={value}
        onChange={setValue}
        multiple={multiple}
        createLabel={createLabel}
        ariaLabel="Picker"
      />
      <output data-testid="value">{value.join(",")}</output>
    </>
  );
}

const OPTS: ComboboxOption[] = [
  { value: "red", label: "Red" },
  { value: "blue", label: "Blue" },
  { value: "green", label: "Green" },
];

describe("Combobox", () => {
  it("selects an existing option from the filtered list", async () => {
    render(withProviders(<Harness multiple options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "bl");
    await userEvent.click(screen.getByRole("option", { name: /Blue/ }));
    expect(screen.getByTestId("value")).toHaveTextContent("blue");
  });

  it("shows no create button when createLabel is not set", async () => {
    render(withProviders(<Harness multiple options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "purple");
    expect(screen.queryByRole("button", { name: /create/i })).not.toBeInTheDocument();
  });

  it("creates a new value through the create button and name input", async () => {
    render(withProviders(<Harness multiple createLabel="Create new tag" options={OPTS} />, { withRouter: false }));
    await userEvent.click(screen.getByRole("button", { name: /create new tag/i }));
    await userEvent.type(screen.getByLabelText("Create new tag"), "purple");
    await userEvent.click(screen.getByRole("button", { name: /^add$/i }));
    expect(screen.getByTestId("value")).toHaveTextContent("purple");
  });

  it("toggles multiple values with the option checkbox and keeps the list open", async () => {
    render(withProviders(<Harness multiple options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "red");
    await userEvent.click(screen.getByRole("option", { name: /Red/ }));
    await userEvent.clear(screen.getByLabelText("Picker"));
    await userEvent.type(screen.getByLabelText("Picker"), "green");
    await userEvent.click(screen.getByRole("option", { name: /Green/ }));
    expect(screen.getByTestId("value")).toHaveTextContent("red,green");
    await userEvent.click(screen.getByRole("button", { name: /remove red/i }));
    expect(screen.getByTestId("value")).toHaveTextContent("green");
  });

  it("keeps a selected option visible and checked, and un-checks it on click", async () => {
    render(withProviders(<Harness multiple options={OPTS} initial={["red"]} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "red");
    const option = screen.getByRole("option", { name: /Red/ });
    expect(option).toHaveAttribute("aria-selected", "true");
    await userEvent.click(option);
    expect(screen.getByTestId("value")).toHaveTextContent("");
  });

  it("single-select replaces the previous value", async () => {
    render(withProviders(<Harness options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "red");
    await userEvent.click(screen.getByRole("option", { name: /Red/ }));
    expect(screen.getByTestId("value")).toHaveTextContent("red");
    await userEvent.type(screen.getByLabelText("Picker"), "blue");
    await userEvent.click(screen.getByRole("option", { name: /Blue/ }));
    expect(screen.getByTestId("value")).toHaveTextContent("blue");
  });
});
