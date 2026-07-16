import { test, expect } from "@playwright/test";
import { API, ADMIN_TOKEN } from "./config";

// Server-side conversion pixels against the real backend: a GA4 pixel stores its
// api_secret masked, never returning the plaintext on read.
const AUTH = { "x-admin-token": ADMIN_TOKEN, "content-type": "application/json" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript((t) => localStorage.setItem("quark_admin_token", t), ADMIN_TOKEN);
});

test("pixels page renders", async ({ page }) => {
  await page.goto("/pixels");
  await expect(page).toHaveURL(/\/pixels/);
  await expect(page.getByRole("link", { name: "Pixels" })).toBeVisible();
});

test("a GA4 pixel stores its api_secret masked, then is removed", async ({ page }) => {
  await page.goto("/pixels");

  const secret = "super-secret-ga4-value";
  const created = await page.request.post(`${API}/admin/pixels`, {
    headers: AUTH,
    data: {
      provider: "ga4",
      credentials: { measurement_id: "G-E2E12345", api_secret: secret },
    },
  });
  expect(created.ok()).toBeTruthy();
  const pixel = await created.json();
  expect(pixel.id).toBeTruthy();

  // The listing masks the secret (measurement_id stays clear, api_secret does not).
  const list = await page.request.get(`${API}/admin/pixels`, { headers: AUTH });
  const body = await list.json();
  expect(JSON.stringify(body)).not.toContain(secret);
  const mine = body.pixels.find((p: { id: number }) => p.id === pixel.id);
  expect(mine.credentials.measurement_id).toBe("G-E2E12345");

  const del = await page.request.delete(`${API}/admin/pixels/${pixel.id}`, { headers: AUTH });
  expect(del.ok()).toBeTruthy();
});
