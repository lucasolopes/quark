import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AppLinks } from "./AppLinks";
import { withProviders } from "@/test-utils";

describe("AppLinks", () => {
  beforeEach(() => {
    localStorage.setItem("quark_admin_token", "s");
    vi.restoreAllMocks();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 404 }));
  });

  it("live JSON validity toggles the Save button in the AASA editor", async () => {
    const user = userEvent.setup();
    render(withProviders(<AppLinks />, { withRouter: false }));

    const region = await screen.findByRole("region", { name: /apple-app-site-association/i });
    const editor = within(region).getByRole("textbox");
    const save = within(region).getByRole("button", { name: /^save$/i });

    await user.type(editor, "not json");
    expect(within(region).getByText(/invalid json/i)).toBeInTheDocument();
    expect(save).toBeDisabled();

    await user.clear(editor);
    await user.type(editor, '{{"applinks":{{}}');
    expect(within(region).queryByText(/invalid json/i)).not.toBeInTheDocument();
    expect(save).toBeEnabled();
  });
});
