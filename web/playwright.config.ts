import { defineConfig, devices } from "@playwright/test";

// The panel runs on :5173 and talks to the quark backend on :8080. Both are
// `localhost`, so they are same-site (ports do not affect site), which is why
// the SameSite=Lax session cookie is sent cross-origin without a proxy. The
// backend and the Keycloak IdP are brought up by global-setup (see e2e/README).
const PANEL = "http://localhost:5173";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: [["list"], ["html", { open: "never" }]],
  globalSetup: "./e2e/global-setup.ts",
  globalTeardown: "./e2e/global-teardown.ts",
  use: {
    baseURL: PANEL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "vite --port 5173 --strictPort",
    url: PANEL,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    env: { VITE_API_BASE_URL: "http://localhost:8080" },
  },
});
