import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CreateWorkspaceForm, SLOW_CREATE_NOTICE_MS } from "./CreateWorkspaceForm";
import { withProviders } from "@/test-utils";

describe("CreateWorkspaceForm", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("derives the slug from the name and posts both", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ id: 1, name: "My Team", slug: "my-team", created: 1 }), { status: 200 }),
    );
    const onCreated = vi.fn();
    render(withProviders(<CreateWorkspaceForm onCreated={onCreated} />));
    await userEvent.type(screen.getByLabelText(/workspace name/i), "My Team");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    const init = spy.mock.calls.find((c) => String(c[0]).includes("/admin/tenants"))?.[1];
    expect(JSON.parse(String(init?.body))).toEqual({ name: "My Team", slug: "my-team" });
    expect(onCreated).toHaveBeenCalled();
  });

  it("shows the slug-taken message on 409 and does not call onCreated", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    const onCreated = vi.fn();
    render(withProviders(<CreateWorkspaceForm onCreated={onCreated} />));
    await userEvent.type(screen.getByLabelText(/workspace name/i), "Acme");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    expect(await screen.findByText(/slug is already taken/i)).toBeInTheDocument();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it("shows a rate-limit message on 429", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 429 }));
    render(withProviders(<CreateWorkspaceForm />));
    await userEvent.type(screen.getByLabelText(/workspace name/i), "Acme");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    expect(await screen.findByText(/too many requests/i)).toBeInTheDocument();
  });

  it("links the error to the slug input via aria-describedby", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    render(withProviders(<CreateWorkspaceForm />));
    const slugInput = screen.getByLabelText(/slug/i);
    expect(slugInput).not.toHaveAttribute("aria-invalid", "true");
    await userEvent.type(screen.getByLabelText(/workspace name/i), "Acme");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    const error = await screen.findByRole("alert");
    expect(slugInput).toHaveAttribute("aria-invalid", "true");
    expect(slugInput).toHaveAttribute("aria-describedby", error.id);
  });

  it("explains that the workspace sign-in is being prepared while creating", async () => {
    // Never settles: the form stays pending for the whole assertion.
    vi.spyOn(globalThis, "fetch").mockReturnValue(new Promise<Response>(() => {}));
    render(withProviders(<CreateWorkspaceForm />));
    expect(screen.queryByText(/sign-in for your workspace/i)).not.toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(/workspace name/i), "Acme");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    expect(await screen.findByText(/sign-in for your workspace/i)).toBeInTheDocument();
    // No fabricated progress: the slow notice only shows once the threshold passes.
    expect(screen.queryByText(/taking longer than usual/i)).not.toBeInTheDocument();
  });

  it("warns that it is taking longer than usual once the threshold passes", async () => {
    // Only the timer APIs the notice uses: faking the rest would stall the
    // microtasks that flip the mutation into its pending state.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      vi.spyOn(globalThis, "fetch").mockReturnValue(new Promise<Response>(() => {}));
      render(withProviders(<CreateWorkspaceForm />));
      // fireEvent, not userEvent: userEvent's own delays fight the fake clock,
      // and this test is about the threshold, not about typing behaviour.
      fireEvent.change(screen.getByLabelText(/workspace name/i), { target: { value: "Acme" } });
      // TanStack Query's notifyManager schedules with `setTimeout(cb, 0)`, so
      // under a fake clock the pending state only lands once time moves.
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /create workspace/i }));
        vi.advanceTimersByTime(0);
      });
      expect(screen.getByText(/sign-in for your workspace/i)).toBeInTheDocument();
      expect(screen.queryByText(/taking longer than usual/i)).not.toBeInTheDocument();

      await act(async () => { vi.advanceTimersByTime(SLOW_CREATE_NOTICE_MS); });
      expect(screen.getByText(/taking longer than usual/i)).toBeInTheDocument();
      // The reassurance that makes the notice actionable rather than alarming.
      expect(screen.getByText(/reloading the page is safe/i)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
