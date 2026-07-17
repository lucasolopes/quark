import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { AcceptInvite } from "./AcceptInvite";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

function wrap(token = "tok_x") {
  return withProviders(
    <MemoryRouter initialEntries={[`/invite/${token}`]}>
      <Routes>
        <Route path="/invite/:token" element={<AcceptInvite />} />
        <Route path="/links" element={<div>LINKS PAGE</div>} />
        <Route path="/login" element={<div>LOGIN PAGE</div>} />
      </Routes>
    </MemoryRouter>,
    { withRouter: false },
  );
}

describe("AcceptInvite", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("unauthenticated: shows a sign-in state and never posts accept", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ authenticated: false, oidc_enabled: true }),
    );
    render(wrap());
    expect(await screen.findByText(/sign in first/i)).toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([, init]) => init?.method === "POST")).toBe(false);
  });

  it("unauthenticated: the sign-in button navigates to /login", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ authenticated: false, oidc_enabled: true }));
    render(wrap());
    await userEvent.click(await screen.findByRole("button"));
    expect(await screen.findByText("LOGIN PAGE")).toBeInTheDocument();
  });

  it("authenticated: shows the accept card and button", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ authenticated: true, oidc_enabled: true }));
    render(wrap());
    expect(await screen.findByRole("button", { name: /accept invite/i })).toBeInTheDocument();
  });

  it("accepting posts to /admin/invites/:token/accept and navigates into the app on success", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse({ authenticated: true, oidc_enabled: true }));
      if (url.includes("/accept") && init?.method === "POST") {
        return Promise.resolve(jsonResponse({ tenant_id: 3, role: "member" }));
      }
      return Promise.resolve(jsonResponse({}));
    });
    render(wrap("tok_x"));
    await userEvent.click(await screen.findByRole("button", { name: /accept invite/i }));

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
      expect(postCall).toBeDefined();
      expect(String(postCall?.[0])).toContain("/admin/invites/tok_x/accept");
    });
    expect(await screen.findByText("LINKS PAGE")).toBeInTheDocument();
  });

  it("403 shows the email-mismatch message", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse({ authenticated: true, oidc_enabled: true }));
      if (init?.method === "POST") return Promise.resolve(new Response("forbidden", { status: 403 }));
      return Promise.resolve(jsonResponse({}));
    });
    render(wrap());
    await userEvent.click(await screen.findByRole("button", { name: /accept invite/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/different email/i);
  });

  it("409 shows the already-member message with a link into the app", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse({ authenticated: true, oidc_enabled: true }));
      if (init?.method === "POST") return Promise.resolve(new Response("conflict", { status: 409 }));
      return Promise.resolve(jsonResponse({}));
    });
    render(wrap());
    await userEvent.click(await screen.findByRole("button", { name: /accept invite/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/already a member/i);
    expect(screen.getByRole("button", { name: /go to your links/i })).toBeInTheDocument();
  });

  it("404 shows the expired/invalid message", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse({ authenticated: true, oidc_enabled: true }));
      if (init?.method === "POST") return Promise.resolve(new Response("not found", { status: 404 }));
      return Promise.resolve(jsonResponse({}));
    });
    render(wrap());
    await userEvent.click(await screen.findByRole("button", { name: /accept invite/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/expired/i);
  });
});
