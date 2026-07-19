import { describe, it, expect } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Combobox, type ComboboxOption } from "./Combobox";
import { withProviders } from "@/test-utils";

function Harness({
  multiple,
  creatable,
  options,
  initial = [],
}: {
  multiple?: boolean;
  creatable?: boolean;
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
        creatable={creatable}
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
    await userEvent.click(screen.getByRole("option", { name: "Blue" }));
    expect(screen.getByTestId("value")).toHaveTextContent("blue");
  });

  it("does not offer create when not creatable", async () => {
    render(withProviders(<Harness multiple options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "purple");
    expect(screen.queryByRole("option", { name: /create/i })).not.toBeInTheDocument();
  });

  it("creates a new value when creatable", async () => {
    render(withProviders(<Harness multiple creatable options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "purple{Enter}");
    expect(screen.getByTestId("value")).toHaveTextContent("purple");
  });

  it("selects multiple values and removes one via its chip", async () => {
    render(withProviders(<Harness multiple options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "red{Enter}");
    await userEvent.type(screen.getByLabelText("Picker"), "green{Enter}");
    expect(screen.getByTestId("value")).toHaveTextContent("red,green");
    await userEvent.click(screen.getByRole("button", { name: /remove red/i }));
    expect(screen.getByTestId("value")).toHaveTextContent("green");
  });

  it("single-select replaces the previous value", async () => {
    render(withProviders(<Harness options={OPTS} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "red{Enter}");
    expect(screen.getByTestId("value")).toHaveTextContent("red");
    await userEvent.type(screen.getByLabelText("Picker"), "blue{Enter}");
    expect(screen.getByTestId("value")).toHaveTextContent("blue");
  });

  it("does not re-add an already selected option", async () => {
    render(withProviders(<Harness multiple options={OPTS} initial={["red"]} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText("Picker"), "red");
    // "Red" is already selected, so it is filtered out of the list.
    expect(screen.queryByRole("option", { name: "Red" })).not.toBeInTheDocument();
  });
});
