import { test, expect } from "@playwright/test";
import { API, ADMIN_TOKEN } from "./config";

// The break-glass admin token reaches the panel against the real backend, and
// the redirect server resolves a created code end to end. The rich UI dialog
// flows are covered by the Vitest component suite; here the value is the real
// backend and the real 302.

test.beforeEach(async ({ page }) => {
  await page.addInitScript((tok) => localStorage.setItem("quark_admin_token", tok), ADMIN_TOKEN);
});

test("token session reaches the authenticated panel", async ({ page }) => {
  await page.goto("/links");
  // RequireAuth passed against the real backend (no bounce to /login).
  await expect(page).toHaveURL(/\/links/);
  // The authenticated shell rendered (a sidebar nav item is present).
  await expect(page.getByRole("link", { name: "Webhooks" })).toBeVisible();
});

test("created link redirects with a 302 end to end", async ({ page }) => {
  await page.goto("/links");
  const created = await page.request.post(`${API}/`, {
    headers: { "x-admin-token": ADMIN_TOKEN, "content-type": "application/json" },
    data: { url: "https://example.com/e2e-redirect" },
  });
  expect(created.ok()).toBeTruthy();
  const { code } = await created.json();
  expect(code).toBeTruthy();

  const redirect = await page.request.get(`${API}/${code}`, { maxRedirects: 0 });
  expect(redirect.status()).toBe(302);
  expect(redirect.headers()["location"]).toBe("https://example.com/e2e-redirect");
});

test("a link to an internal destination is refused (SSRF guard)", async ({ page }) => {
  await page.goto("/links");
  const res = await page.request.post(`${API}/`, {
    headers: { "x-admin-token": ADMIN_TOKEN, "content-type": "application/json" },
    data: { url: "http://127.0.0.1:8080/admin/me" },
  });
  // The SSRF guard rejects internal/loopback destinations.
  expect(res.status()).toBe(403);
});
