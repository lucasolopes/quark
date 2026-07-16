import { test, expect } from "@playwright/test";
import { API, ADMIN_TOKEN } from "./config";

// Testing the Google Sheets connector against the REAL Google provider is a
// manual smoke test, not part of CI: Google's consent screen blocks automated
// logins (bot detection, 2FA, consent), so a headless Playwright run cannot
// drive it. This file documents the operator steps and keeps a light
// programmatic check for once the connector is wired up and connected by hand.
// It is skipped unless QUARK_E2E_SHEETS=1 is set.
//
// Prerequisites (operator, one time):
//   1. Google Cloud project; enable the Google Sheets API and the Google Drive
//      API.
//   2. OAuth consent screen (External), scope
//      https://www.googleapis.com/auth/drive.file. Add your Google account as a
//      test user while the app is in testing.
//   3. OAuth 2.0 Client ID (type "Web application"), Authorized redirect URI:
//      https://<your-quark-host>/admin/integrations/sheets/callback
//   4. Run quark behind TLS (a real host or an https tunnel, since Google
//      requires an https redirect) with:
//        QUARK_SHEETS_CLIENT_ID=<client id>
//        QUARK_SHEETS_CLIENT_SECRET=<client secret>
//        QUARK_SHEETS_REDIRECT_URL=https://<your-quark-host>/admin/integrations/sheets/callback
//        QUARK_SHEETS_SYNC_SECS=<optional interval>
//
// Manual verification (operator):
//   - Open the panel, go to Extensions, click "Connect Google Sheets", complete
//     Google's consent, and confirm the card shows the connected email.
//   - Click "Sync now" and confirm a new spreadsheet appears in your Drive with
//     one row per link.
//   - Then run this spec with QUARK_E2E_SHEETS=1 to assert the status endpoint
//     reports connected.

const RUN = process.env.QUARK_E2E_SHEETS === "1";

test.describe("real Google Sheets connector (manual)", () => {
  test.skip(!RUN, "set QUARK_E2E_SHEETS=1 and connect a real Google account by hand to run");

  test("status reports connected once the operator has connected", async ({ request }) => {
    // The connect itself is manual (Google blocks automation); this only checks
    // that, after connecting by hand, quark reports the connection.
    const res = await request.get(`${API}/admin/integrations/sheets/status`, {
      headers: { "x-admin-token": ADMIN_TOKEN },
    });
    expect(res.ok()).toBe(true);
    const body = await res.json();
    expect(body.connected).toBe(true);
  });
});
