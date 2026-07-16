import { test, expect, type Page } from "@playwright/test";
import { API, ADMIN_USER, READER_USER } from "./config";

// Drives the full OIDC Authorization Code + PKCE handshake against the seeded
// Keycloak realm: click the provider button, authenticate at the IdP, and land
// back in the panel with a server-side session. This exercises the real code
// path (discover, authorize, exchange_code, verify_id_token, map_scopes) that
// unit and component tests cannot reach.

async function signInWithProvider(page: Page, user: { username: string; password: string }) {
  await page.goto("/login");
  const providerBtn = page.getByRole("button", { name: "Sign in with provider" });
  await expect(providerBtn).toBeVisible();
  await providerBtn.click();

  // Keycloak's login page.
  await page.waitForURL(/localhost:8081\/realms\/quark\/protocol\/openid-connect/);
  await page.fill("#username", user.username);
  await page.fill("#password", user.password);
  await page.click("#kc-login");

  // Back on the panel; the Login screen auto-navigates to /links once the
  // session cookie is present.
  await page.waitForURL(/localhost:5173\/(links)?$/, { timeout: 15_000 });
}

test("admin group signs in and gets a full-scope session", async ({ page }) => {
  await signInWithProvider(page, ADMIN_USER);
  await page.waitForURL(/\/links/);

  const me = await page.request.get(`${API}/admin/me`);
  const body = await me.json();
  expect(body.authenticated).toBe(true);
  expect(body.display).toBe(ADMIN_USER.username);
  expect(body.scopes).toContain("full");

  // A full-scope session can create a link (the CSRF header is sent by the app;
  // here we send it explicitly on the direct request).
  const created = await page.request.post(`${API}/`, {
    headers: { "content-type": "application/json", "x-quark-csrf": "1" },
    data: { url: "https://example.com/from-oidc-admin" },
  });
  expect(created.ok()).toBeTruthy();
});

test("reader group gets a read-only session, not full", async ({ page }) => {
  await signInWithProvider(page, READER_USER);

  const me = await page.request.get(`${API}/admin/me`);
  const body = await me.json();
  expect(body.authenticated).toBe(true);
  expect(body.display).toBe(READER_USER.username);
  expect(body.scopes).toContain("links_read");
  expect(body.scopes).not.toContain("full");

  // Read-only cannot create: links_write is not covered -> 403.
  const created = await page.request.post(`${API}/`, {
    headers: { "content-type": "application/json", "x-quark-csrf": "1" },
    data: { url: "https://example.com/reader-should-fail" },
  });
  expect(created.status()).toBe(403);
});

test("logout revokes the session", async ({ page }) => {
  await signInWithProvider(page, ADMIN_USER);

  // The panel's logout sends the x-quark-csrf header the server requires.
  const out = await page.request.post(`${API}/admin/logout`, {
    headers: { "x-quark-csrf": "1" },
  });
  expect(out.ok()).toBeTruthy();

  const me = await page.request.get(`${API}/admin/me`);
  const body = await me.json();
  expect(body.authenticated).toBe(false);
});
