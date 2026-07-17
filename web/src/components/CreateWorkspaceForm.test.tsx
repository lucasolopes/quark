import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CreateWorkspaceForm } from "./CreateWorkspaceForm";
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
});
