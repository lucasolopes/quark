import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
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
};

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
