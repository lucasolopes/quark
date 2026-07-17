import { describe, it, expect, beforeEach, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LinkTable } from "./LinkTable";
import { withProviders } from "@/test-utils";
import type { Link } from "@/lib/types";

const link: Link = {
  id: 1,
  code: "6lB362J",
  url: "https://example.com/a-pretty-long-page",
  expiry: null,
  created: 1700000000,
  tags: [],
  visits: 0,
  rules: [],
  variants: [],
};

function meResponse(body: object) {
  return new Response(JSON.stringify(body), { status: 200 });
}
const ossMe = { authenticated: true, oidc_enabled: false };
const cloudMe = {
  authenticated: true,
  oidc_enabled: false,
  current_tenant: 1,
  memberships: [{ tenant_id: 1, name: "Acme", slug: "acme", role: "Owner" }],
  tenant_domain_suffix: "quark.link",
};

beforeEach(() => {
  vi.restoreAllMocks();
  // Default: OSS `me` (no memberships/suffix), so pre-existing tests that
  // don't care about the short URL keep behaving as before this feature.
  vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse(ossMe));
});

/**
 * Renders `LinkTable` and waits for the `["me"]` query to settle before
 * returning, so a test's first click sees the resolved tenant slug/suffix
 * rather than racing the still-pending fetch (which would otherwise flip
 * the short URL under a Radix trigger mid-interaction and drop the click).
 */
async function renderTable(links: Link[], me: object = ossMe) {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse(me));
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(withProviders(<LinkTable links={links} onEdit={() => {}} onDelete={() => {}} />, { queryClient }));
  await waitFor(() => expect(queryClient.getQueryData(["me"])).toEqual(me));
}

describe("LinkTable — A/B variants badge", () => {
  it("shows a badge with the variant count when the link has variants", () => {
    const linkWithVariants: Link = {
      ...link,
      variants: [
        { url: "https://variant-a.com", weight: 1 },
        { url: "https://variant-b.com", weight: 1 },
      ],
    };
    render(withProviders(<LinkTable links={[linkWithVariants]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("A/B: 2")).toBeInTheDocument();
  });

  it("shows no badge when the link has no variants", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.queryByText(/^A\/B:/)).not.toBeInTheDocument();
  });
});

describe("LinkTable — health indicator", () => {
  it("shows a broken indicator (with the status) when the destination is broken", () => {
    const broken: Link = { ...link, health: { healthy: false, status: 404, checked_at: 1700000000 } };
    render(withProviders(<LinkTable links={[broken]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByRole("img", { name: /broken \(HTTP 404\)/i })).toBeInTheDocument();
  });

  it("shows a reachable indicator when the destination is healthy", () => {
    const healthy: Link = { ...link, health: { healthy: true, status: 200, checked_at: 1700000000 } };
    render(withProviders(<LinkTable links={[healthy]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByRole("img", { name: /reachable/i })).toBeInTheDocument();
  });

  it("shows no health indicator when the link was never checked", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.queryByRole("img", { name: /reachable|broken/i })).not.toBeInTheDocument();
  });
});

describe("LinkTable — QR code", () => {
  it("opens the QR code dialog with the short URL (OSS fallback) and the download button", async () => {
    await renderTable([link]);

    await userEvent.click(screen.getByRole("button", { name: /more actions for 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));

    expect(await screen.findByRole("dialog", { name: /qr code for 6lB362J/i })).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/6lB362J`)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /download png/i })).toBeInTheDocument();
  });

  it("closes the QR code dialog on cancel", async () => {
    await renderTable([link]);

    await userEvent.click(screen.getByRole("button", { name: /more actions for 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));
    await screen.findByRole("dialog", { name: /qr code for 6lB362J/i });

    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByRole("dialog", { name: /qr code for 6lB362J/i })).not.toBeInTheDocument();
  });

  it("shows the tenant subdomain URL when the cloud tenant has a provisioned suffix", async () => {
    await renderTable([link], cloudMe);

    await userEvent.click(screen.getByRole("button", { name: /more actions for 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));

    expect(await screen.findByRole("dialog", { name: /qr code for 6lB362J/i })).toBeInTheDocument();
    expect(screen.getByText("https://acme.quark.link/6lB362J")).toBeInTheDocument();
  });
});

describe("LinkTable — copy short URL", () => {
  beforeEach(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
  });

  it("copies the PUBLIC_BASE URL in OSS (no tenant suffix)", async () => {
    await renderTable([link]);
    await userEvent.click(screen.getByRole("button", { name: /copy short link for 6lB362J/i }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(`${window.location.origin}/6lB362J`);
  });

  it("copies the tenant subdomain URL when the cloud tenant has a provisioned suffix", async () => {
    await renderTable([link], cloudMe);
    await userEvent.click(screen.getByRole("button", { name: /copy short link for 6lB362J/i }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("https://acme.quark.link/6lB362J");
  });
});

describe("LinkTable — tags", () => {
  it("renders tags as badges", () => {
    const tagged: Link = { ...link, tags: ["promo", "summer"] };
    render(withProviders(<LinkTable links={[tagged]} onEdit={() => {}} onDelete={() => {}} />));

    expect(screen.getByText("promo")).toBeInTheDocument();
    expect(screen.getByText("summer")).toBeInTheDocument();
  });

  it("collapses extra tags into a +k badge", () => {
    const tagged: Link = { ...link, tags: ["a", "b", "c", "d", "e"] };
    render(withProviders(<LinkTable links={[tagged]} onEdit={() => {}} onDelete={() => {}} />));

    expect(screen.getByText("a")).toBeInTheDocument();
    expect(screen.getByText("b")).toBeInTheDocument();
    expect(screen.getByText("c")).toBeInTheDocument();
    expect(screen.queryByText("d")).not.toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();
  });

  it("shows a dash when a link has no tags", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });
});

describe("LinkTable — visits", () => {
  it("shows visits/max when max_visits is set", () => {
    const limited: Link = { ...link, visits: 12, max_visits: 100 };
    render(withProviders(<LinkTable links={[limited]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("12 / 100")).toBeInTheDocument();
  });

  it("shows only the visit count when there is no max_visits", () => {
    const unlimited: Link = { ...link, visits: 7 };
    render(withProviders(<LinkTable links={[unlimited]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("7")).toBeInTheDocument();
  });
});

describe("LinkTable — redirect rules badge", () => {
  it("shows a rule-count badge when the link has rules", () => {
    const linkWithRules: Link = {
      ...link,
      rules: [{ field: "country", values: ["BR"], to: "https://example.com/br" }],
    };
    render(withProviders(<LinkTable links={[linkWithRules]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("1 rules")).toBeInTheDocument();
  });

  it("shows no badge when the link has no rules", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.queryByText(/rules?$/)).not.toBeInTheDocument();
  });
});
