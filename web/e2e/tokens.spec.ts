import { test, expect } from "@playwright/test";
import { API, ADMIN_TOKEN } from "./config";

// API tokens against the real backend: a scoped token is returned once and then
// enforces its scope (a read-only token lists links but cannot create one).
const AUTH = { "x-admin-token": ADMIN_TOKEN, "content-type": "application/json" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript((t) => localStorage.setItem("quark_admin_token", t), ADMIN_TOKEN);
});

test("tokens page renders", async ({ page }) => {
  await page.goto("/tokens");
  await expect(page).toHaveURL(/\/tokens/);
  await expect(page.getByRole("link", { name: "API tokens" })).toBeVisible();
});

test("a read-only token lists links but cannot create, then is revoked", async ({ page }) => {
  await page.goto("/tokens");

  const created = await page.request.post(`${API}/admin/tokens`, {
    headers: AUTH,
    data: { name: "e2e-readonly", scopes: ["links_read"] },
  });
  expect(created.ok()).toBeTruthy();
  const { id, token } = await created.json();
  expect(token).toBeTruthy();

  const scoped = { "x-admin-token": token, "content-type": "application/json" };

  // links_read covers listing.
  const list = await page.request.get(`${API}/admin/links?limit=1`, { headers: scoped });
  expect(list.ok()).toBeTruthy();

  // links_read does NOT cover create -> 403.
  const create = await page.request.post(`${API}/`, {
    headers: scoped,
    data: { url: "https://example.com/should-fail" },
  });
  expect(create.status()).toBe(403);

  // Revoke it; the token no longer authenticates.
  const del = await page.request.delete(`${API}/admin/tokens/${id}`, { headers: AUTH });
  expect(del.ok()).toBeTruthy();
  const after = await page.request.get(`${API}/admin/links?limit=1`, { headers: scoped });
  expect(after.status()).toBe(401);
});
