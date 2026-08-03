import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { withProviders } from "@/test-utils";
import { DeleteWorkspaceDialog } from "./DeleteWorkspaceDialog";

function open() {
  return withProviders(
    <DeleteWorkspaceDialog open onOpenChange={() => {}} tenantId={1} name="Acme" slug="acme" />,
  );
}

describe("DeleteWorkspaceDialog", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("keeps the confirm button disabled until the typed slug matches exactly", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    render(open());
    const confirm = await screen.findByRole("button", { name: /^delete workspace$/i });
    expect(confirm).toBeDisabled();

    const field = screen.getByLabelText(/to confirm/i);
    // A prefix is not a match.
    await userEvent.type(field, "acm");
    expect(confirm).toBeDisabled();
    // Neither is a different case.
    await userEvent.clear(field);
    await userEvent.type(field, "ACME");
    expect(confirm).toBeDisabled();
    // Nor a superstring.
    await userEvent.clear(field);
    await userEvent.type(field, "acmex");
    expect(confirm).toBeDisabled();

    await userEvent.clear(field);
    await userEvent.type(field, "acme");
    expect(confirm).toBeEnabled();
    expect(spy).not.toHaveBeenCalled();
  });

  it("calls the delete endpoint when the typed slug matches", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    const onOpenChange = vi.fn();
    render(withProviders(
      <DeleteWorkspaceDialog open onOpenChange={onOpenChange} tenantId={7} name="Acme" slug="acme" />,
    ));
    await userEvent.type(await screen.findByLabelText(/to confirm/i), "acme");
    await userEvent.click(screen.getByRole("button", { name: /^delete workspace$/i }));
    await waitFor(() => {
      const call = spy.mock.calls.find((c) => String(c[0]).includes("/admin/tenants/7"));
      expect(call).toBeTruthy();
      expect(String(call?.[1]?.method).toUpperCase()).toBe("DELETE");
    });
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("shows the last-workspace message on 409", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    render(open());
    await userEvent.type(await screen.findByLabelText(/to confirm/i), "acme");
    await userEvent.click(screen.getByRole("button", { name: /^delete workspace$/i }));
    expect(await screen.findByText(/last workspace/i)).toBeInTheDocument();
  });

  it("shows the owner-only message on 403", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 403 }));
    render(open());
    await userEvent.type(await screen.findByLabelText(/to confirm/i), "acme");
    await userEvent.click(screen.getByRole("button", { name: /^delete workspace$/i }));
    expect(await screen.findByText(/only the workspace owner/i)).toBeInTheDocument();
  });

  it("falls back to the generic message on any other failure", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 503 }));
    render(open());
    await userEvent.type(await screen.findByLabelText(/to confirm/i), "acme");
    await userEvent.click(screen.getByRole("button", { name: /^delete workspace$/i }));
    expect(await screen.findByText(/could not delete the workspace/i)).toBeInTheDocument();
  });
});
