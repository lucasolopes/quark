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

  it("pre-populates the tags field with the link's current tags", () => {
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));
    expect(screen.getByLabelText(/tags/i)).toHaveValue("promo, summer");
  });

  it("sends the edited tags array on submit", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));

    const tagsField = screen.getByLabelText(/tags/i);
    await userEvent.clear(tagsField);
    await userEvent.type(tagsField, "promo, winter");
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

  it("pre-populates the folder field from the link's folder", () => {
    const l = makeLink({ folder: "Marketing" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));
    expect(screen.getByLabelText(/folder/i)).toHaveValue("Marketing");
  });

  it("sends folder: null when a pre-filled folder is cleared", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const l = makeLink({ folder: "Marketing" });
    render(withProviders(<EditLinkDialog link={l} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.clear(screen.getByLabelText(/folder/i));
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

    await userEvent.type(screen.getByLabelText(/folder/i), "Docs");
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

    await userEvent.type(screen.getByLabelText(/ios/i), "https://apps.apple.com/app/y");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("app_ios", "https://apps.apple.com/app/y");
  });
});
