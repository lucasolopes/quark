import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { Link2, Radio } from "lucide-react";
import { Route, Routes, useLocation } from "react-router-dom";
import type { NavGroup } from "@/app/Shell";
import { MobileNav } from "./MobileNav";
import { withProviders } from "@/test-utils";

const groups: NavGroup[] = [
  {
    label: "Links group",
    items: [{ to: "/links", label: "Links", icon: Link2, show: true }],
  },
  {
    label: "Analytics group",
    items: [{ to: "/pixels", label: "Pixels", icon: Radio, show: true }],
  },
];

/** Shows the current route's pathname, so a NavLink click is observable. */
function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location-probe">{location.pathname}</div>;
}

function renderMobileNav(overrides: { open?: boolean; groups?: NavGroup[]; footer?: ReactNode; children?: ReactNode } = {}) {
  const onOpenChange = vi.fn();
  render(
    withProviders(
      <>
        <Routes>
          <Route path="/links" element={<LocationProbe />} />
          <Route path="/pixels" element={<LocationProbe />} />
        </Routes>
        <MobileNav
          open={overrides.open ?? true}
          onOpenChange={onOpenChange}
          groups={overrides.groups ?? groups}
          footer={overrides.footer}
        >
          {overrides.children}
        </MobileNav>
      </>,
      { initialEntries: ["/links"] },
    ),
  );
  return { onOpenChange };
}

describe("MobileNav", () => {
  it("renders the group labels and item labels", () => {
    renderMobileNav();
    expect(screen.getByText("Links group")).toBeInTheDocument();
    expect(screen.getByText("Analytics group")).toBeInTheDocument();
    expect(screen.getByText("Links")).toBeInTheDocument();
    expect(screen.getByText("Pixels")).toBeInTheDocument();
  });

  it("does not render anything when closed", () => {
    renderMobileNav({ open: false });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("clicking a nav item navigates and closes the drawer", async () => {
    const { onOpenChange } = renderMobileNav();
    await userEvent.click(screen.getByRole("link", { name: "Pixels" }));
    expect(await screen.findByTestId("location-probe")).toHaveTextContent("/pixels");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("Esc closes the drawer", async () => {
    const { onOpenChange } = renderMobileNav();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(onOpenChange).toHaveBeenCalled());
    expect(onOpenChange.mock.calls.at(-1)?.[0]).toBe(false);
  });

  it("clicking the close button closes the drawer", async () => {
    const { onOpenChange } = renderMobileNav();
    await userEvent.click(screen.getByRole("button", { name: "Close menu" }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalled());
    expect(onOpenChange.mock.calls.at(-1)?.[0]).toBe(false);
  });

  it("renders footer content below the nav groups", () => {
    renderMobileNav({ footer: <div data-testid="my-footer">Footer stuff</div> });
    expect(screen.getByTestId("my-footer")).toBeInTheDocument();
  });

  it("renders children content above the nav groups", () => {
    renderMobileNav({ children: <div data-testid="my-header">Header stuff</div> });
    expect(screen.getByTestId("my-header")).toBeInTheDocument();
  });
});
