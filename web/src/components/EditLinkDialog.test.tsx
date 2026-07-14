import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EditLinkDialog } from "./EditLinkDialog";
import { withProviders } from "@/test-utils";
import type { Link } from "@/lib/types";

const link: Link = {
  id: 1,
  code: "6lB362J",
  url: "https://example.com",
  expiry: null,
  created: 1700000000,
  tags: ["promo", "summer"],
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
