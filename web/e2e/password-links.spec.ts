import { test, expect } from "@playwright/test";
import { API, ADMIN_TOKEN } from "./config";

// A password-protected link serves an interstitial instead of redirecting, and
// only unlocks (sets the cookie and redirects) after the correct password. This
// exercises the real argon2 verify + unlock-cookie round trip through a browser.
const AUTH = { "x-admin-token": ADMIN_TOKEN, "content-type": "application/json" };
const PASSWORD = "open-sesame";
const TARGET = "https://example.com/protected-destination";

test("protected link shows the interstitial and unlocks only with the right password", async ({
  page,
}) => {
  // A fresh browser context per test, so no unlock cookie leaks across runs.
  const created = await page.request.post(`${API}/`, {
    headers: AUTH,
    data: { url: TARGET, password: PASSWORD },
  });
  expect(created.ok()).toBeTruthy();
  const { code } = await created.json();

  // Bare visit: the interstitial (200 HTML with a password field), not a 302.
  const gate = await page.request.get(`${API}/${code}`, { maxRedirects: 0 });
  expect(gate.status()).toBe(200);
  expect(await gate.text()).toContain('name="password"');

  // Wrong password: still gated, no redirect to the target.
  const wrong = await page.request.post(`${API}/${code}`, {
    form: { password: "not-it" },
    maxRedirects: 0,
  });
  expect([200, 401, 403]).toContain(wrong.status());
  if (wrong.status() >= 300 && wrong.status() < 400) {
    expect(wrong.headers()["location"]).not.toBe(TARGET);
  }

  // Correct password: the server sets the unlock cookie and redirects back to
  // the code (the cookie is stored in this request context).
  const ok = await page.request.post(`${API}/${code}`, {
    form: { password: PASSWORD },
    maxRedirects: 0,
  });
  expect(ok.status()).toBeGreaterThanOrEqual(300);
  expect(ok.status()).toBeLessThan(400);

  // Now the same context resolves the link to the real destination.
  const unlocked = await page.request.get(`${API}/${code}`, { maxRedirects: 0 });
  expect(unlocked.status()).toBe(302);
  expect(unlocked.headers()["location"]).toBe(TARGET);
});
