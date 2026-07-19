import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EditLinkDialog } from "./EditLinkDialog";
import { withProviders } from "@/test-utils";
import type { Link } from "@/lib/types";

const link: Link = {
  id: 1,
  code: "6lB362J",
  url: "https://example.com/a-pretty-long-page",
  expiry: null,
  created: 1700000000,
  tags: ["promo", "summer"],
  max_visits: 100,
  visits: 12,
  rules: [],
  variants: [],
};

function makeLink(overrides: Partial<Link> = {}): Link {
  return {
    id: 1,
    code: "6lB362J",
    url: "https://ok.com",
    expiry: null,
    created: 1700000000,
    tags: [],
    visits: 0,
    rules: [],
    variants: [],
    ...overrides,
  };
}

function patchBody(fetchMock: ReturnType<typeof vi.spyOn>): Record<string, unknown> {
  return JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
}

describe("EditLinkDialog — tags", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("pre-populates the tags picker with the link's current tags as chips", () => {
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));
    expect(screen.getByRole("button", { name: /remove promo/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /remove summer/i })).toBeInTheDocument();
  });

  it("sends the edited tags array on submit", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));

    // Remove the "summer" chip and create "winter" via the create button.
    await userEvent.click(screen.getByRole("button", { name: /remove summer/i }));
    await userEvent.click(screen.getByRole("button", { name: /create new tag/i }));
    await userEvent.type(screen.getByLabelText(/create new tag/i), "winter{Enter}");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init!.body)) as { tags?: string[] };
    expect(body.tags).toEqual(["promo", "winter"]);
  });
});

describe("EditLinkDialog — max visits", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("clears an existing max_visits limit when the field is emptied on save", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /scheduling and limits/i }));
    const maxVisitsInput = screen.getByLabelText(/max visits/i);
    expect(maxVisitsInput).toHaveValue(100);
    await userEvent.clear(maxVisitsInput);
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body.max_visits).toBeNull();
  });

  it("does not send max_visits when the field stays empty and the link had no limit", async () => {
    const unlimited: Link = { ...link, max_visits: undefined };
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    render(withProviders(<EditLinkDialog link={unlimited} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body).not.toHaveProperty("max_visits");
  });
});

describe("EditLinkDialog — folder", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("pre-populates the folder picker from the link's folder", () => {
    const l = makeLink({ folder: "Marketing" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));
    expect(screen.getByRole("button", { name: /remove Marketing/i })).toBeInTheDocument();
  });

  it("sends folder: null when a pre-filled folder is cleared", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink({ folder: "Marketing" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /remove Marketing/i }));
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("folder", null);
  });

  it("omits folder when it was empty and stays empty", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).not.toHaveProperty("folder");
  });

  it("sends the trimmed folder when set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /create new folder/i }));
    await userEvent.type(screen.getByLabelText(/create new folder/i), "Docs{Enter}");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("folder", "Docs");
  });
});

describe("EditLinkDialog — app destinations", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("sends app_ios: null when a pre-filled iOS destination is cleared", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink({ app_ios: "https://apps.apple.com/app/x" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /app redirect/i }));
    await userEvent.clear(screen.getByLabelText(/ios/i));
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("app_ios", null);
    // Android was never set and stays empty → omitted, not null.
    expect(body).not.toHaveProperty("app_android");
  });

  it("omits app fields that were empty and stay empty", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).not.toHaveProperty("app_ios");
    expect(body).not.toHaveProperty("app_android");
  });

  it("sends the trimmed value when an app destination is set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /app redirect/i }));
    await userEvent.type(screen.getByLabelText(/ios/i), "https://apps.apple.com/app/y");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("app_ios", "https://apps.apple.com/app/y");
  });
});

describe("EditLinkDialog — fallback url", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("pre-populates and sends an edited fallback_url", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink({ fallback_url: "https://old.com" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /scheduling and limits/i }));
    const field = screen.getByLabelText(/fallback/i);
    expect(field).toHaveValue("https://old.com");
    await userEvent.clear(field);
    await userEvent.type(field, "https://new.com");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("fallback_url", "https://new.com");
  });

  it("sends fallback_url: null when an existing fallback is cleared", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink({ fallback_url: "https://old.com" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /scheduling and limits/i }));
    await userEvent.clear(screen.getByLabelText(/fallback/i));
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("fallback_url", null);
  });

  it("omits fallback_url when it was empty and stays empty", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).not.toHaveProperty("fallback_url");
  });
});

describe("EditLinkDialog — password", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("sends a new password when set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /^password$/i }));
    await userEvent.type(screen.getByLabelText(/^password$/i), "hunter2");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    expect(patchBody(fetchMock)).toHaveProperty("password", "hunter2");
  });

  it("sends password: null when the remove-protection box is checked", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink({ has_password: true });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /^password$/i }));
    await userEvent.click(screen.getByLabelText(/remove password/i));
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    expect(patchBody(fetchMock)).toHaveProperty("password", null);
  });

  it("omits password when left untouched on an unprotected link", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    expect(patchBody(fetchMock)).not.toHaveProperty("password");
  });
});

describe("EditLinkDialog — click-threshold alert (LUC-66)", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  function mockAlertFetch(currentRule: { threshold: number; window_secs: number } | null = null) {
    return vi.spyOn(globalThis, "fetch").mockImplementation(async (_url, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      if (method === "GET") return new Response(JSON.stringify(currentRule), { status: 200 });
      if (method === "DELETE") return new Response(null, { status: 204 });
      // PUT: echo the body back like the real endpoint does.
      return new Response(String(init?.body ?? "null"), { status: 200 });
    });
  }

  it("does not fetch the alert rule until the section is expanded", () => {
    const fetchMock = mockAlertFetch();
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("shows the current rule once the section is expanded", async () => {
    mockAlertFetch({ threshold: 10, window_secs: 600 });
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /click-threshold alert/i }));

    expect(await screen.findByLabelText(/click threshold/i)).toHaveValue(10);
    expect(screen.getByLabelText(/window \(minutes\)/i)).toHaveValue(10);
  });

  it("saving sends a PUT with threshold and window_secs converted from minutes", async () => {
    const fetchMock = mockAlertFetch(null);
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /click-threshold alert/i }));
    await screen.findByText(/no alert rule set/i);

    await userEvent.type(screen.getByLabelText(/click threshold/i), "5");
    await userEvent.type(screen.getByLabelText(/window \(minutes\)/i), "3");
    await userEvent.click(screen.getByRole("button", { name: /^save alert$/i }));

    await waitFor(() => {
      const putCall = fetchMock.mock.calls.find(([, init]) => (init as RequestInit)?.method === "PUT");
      expect(putCall).toBeDefined();
    });
    const putCall = fetchMock.mock.calls.find(([, init]) => (init as RequestInit)?.method === "PUT")!;
    const [url, init] = putCall;
    expect(String(url)).toContain(`/admin/links/${l.code}/alert`);
    const body = JSON.parse(String((init as RequestInit).body));
    expect(body).toEqual({ threshold: 5, window_secs: 180 });
  });

  it("removing sends a DELETE to the link's alert endpoint", async () => {
    const fetchMock = mockAlertFetch({ threshold: 10, window_secs: 600 });
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /click-threshold alert/i }));
    await screen.findByLabelText(/click threshold/i);

    await userEvent.click(screen.getByRole("button", { name: /^remove alert$/i }));

    await waitFor(() => {
      const delCall = fetchMock.mock.calls.find(([, init]) => (init as RequestInit)?.method === "DELETE");
      expect(delCall).toBeDefined();
    });
    const delCall = fetchMock.mock.calls.find(([, init]) => (init as RequestInit)?.method === "DELETE")!;
    expect(String(delCall[0])).toContain(`/admin/links/${l.code}/alert`);
  });

  it("rejects a threshold below 1 client-side without calling PUT", async () => {
    const fetchMock = mockAlertFetch(null);
    const l = makeLink();
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /click-threshold alert/i }));
    await screen.findByText(/no alert rule set/i);

    await userEvent.type(screen.getByLabelText(/window \(minutes\)/i), "1");
    await userEvent.click(screen.getByRole("button", { name: /^save alert$/i }));

    expect(fetchMock.mock.calls.some(([, init]) => (init as RequestInit)?.method === "PUT")).toBe(false);
  });
});
