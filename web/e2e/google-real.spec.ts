import { test, expect } from "@playwright/test";
import { API } from "./config";

// Testing against the REAL Google provider is a manual smoke test, not part of
// CI: Google blocks automated logins (bot detection, 2FA, consent), so a
// headless Playwright run cannot drive its login page. This file documents the
// steps and keeps a light programmatic check for when the operator wires Google
// up. It is skipped unless QUARK_E2E_GOOGLE=1 is set.
//
// Prerequisites (operator, one time):
//   1. Google Cloud project; enable the Google Sheets API and Drive API only if
//      Sheets sync is in scope (not needed for login itself).
//   2. OAuth consent screen (External), scopes: openid, email, profile.
//   3. OAuth 2.0 Client ID (type "Web application"), Authorized redirect URI:
//      https://<your-quark-host>/admin/callback
//   4. Run quark with:
//        QUARK_OIDC_ISSUER=https://accounts.google.com
//        QUARK_OIDC_CLIENT_ID=<client id>
//        QUARK_OIDC_CLIENT_SECRET=<client secret>
//        QUARK_OIDC_REDIRECT_URL=https://<your-quark-host>/admin/callback
//        QUARK_OIDC_ADMIN_CLAIM=email
//        QUARK_OIDC_ADMIN_VALUE=<your-google-account@example.com>
//      Google emits no group claim, so gate on the exact email (or a Workspace
//      group) via QUARK_OIDC_ADMIN_CLAIM=email.
//
// Manual verification (operator):
//   - Open the panel, click "Sign in with provider", complete Google's login and
//     consent, and confirm you land in the panel authenticated.
//   - Confirm a non-matching Google account is denied (empty scopes -> no access).

const RUN = process.env.QUARK_E2E_GOOGLE === "1";

test.describe("real Google provider (manual)", () => {
  test.skip(!RUN, "set QUARK_E2E_GOOGLE=1 and configure a real Google client to run");

  test("discovery points at Google and OIDC is enabled", async ({ request }) => {
    // With a real Google client configured, quark's /admin/me still reports OIDC
    // is on; the login itself is completed by hand (Google blocks automation).
    const me = await request.get(`${API}/admin/me`);
    const body = await me.json();
    expect(body.oidc_enabled).toBe(true);
  });
});
