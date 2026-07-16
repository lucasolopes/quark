import { test, expect } from "@playwright/test";
import { API, ADMIN_TOKEN } from "./config";

// Webhook CRUD against the real backend: a generic subscription returns a
// signing secret once, and the secret is masked on later reads.
const AUTH = { "x-admin-token": ADMIN_TOKEN, "content-type": "application/json" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript((t) => localStorage.setItem("quark_admin_token", t), ADMIN_TOKEN);
});

test("webhooks page renders for an authenticated operator", async ({ page }) => {
  await page.goto("/webhooks");
  await expect(page).toHaveURL(/\/webhooks/);
  await expect(page.getByRole("link", { name: "Webhooks" })).toBeVisible();
});

test("create returns a signing secret once, list masks it, delete removes it", async ({ page }) => {
  await page.goto("/webhooks");

  const created = await page.request.post(`${API}/admin/webhooks`, {
    headers: AUTH,
    data: { url: "https://example.com/hook", events: ["link.created"], kind: "generic" },
  });
  expect(created.ok()).toBeTruthy();
  const body = await created.json();
  expect(body.id).toBeGreaterThan(0);
  // A generic subscription signs with an HMAC secret, returned once in the clear.
  expect(body.secret).toMatch(/^whsec_/);

  // The listing never returns the secret in the clear again.
  const list = await page.request.get(`${API}/admin/webhooks`, { headers: AUTH });
  const listBody = await list.json();
  const mine = listBody.webhooks.find((w: { id: number }) => w.id === body.id);
  expect(mine).toBeTruthy();
  expect(JSON.stringify(mine)).not.toContain(body.secret.replace("whsec_", ""));

  const del = await page.request.delete(`${API}/admin/webhooks/${body.id}`, { headers: AUTH });
  expect(del.ok()).toBeTruthy();
});
