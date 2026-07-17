import { test, expect, type Page } from "@playwright/test";
import { spawn, execSync, type ChildProcess } from "node:child_process";
import { existsSync, openSync, statSync } from "node:fs";
import { resolve } from "node:path";

// Per-tenant OIDC login E2E (LUC-49). Complements oidc-login.spec.ts (which
// covers the GLOBAL realm on the shared :8080 LMDB instance): here a SECOND
// Keycloak realm ("acme") stands in for a tenant's own IdP, and a dedicated
// CLOUD quark (multi-tenant + Postgres) on :8082 logs a user in via
// `/admin/login?org=acme`, exercising the real per-tenant path — tenant
// resolution from the slug, that realm's OIDC config, the group-claim role
// mapping, and the default-closed required-group gate — against a live IdP,
// which the unit/integration tests cannot reach.
//
// Self-contained: the shared globalSetup starts (and taskkills every quark.exe
// for) the :8080 instance, so this suite must start its OWN :8082 cloud quark
// in beforeAll, AFTER globalSetup has run.

const CLOUD_API = "http://localhost:8082";
const KEYCLOAK = "http://localhost:8081";
const DB = "postgres://quark:quark@localhost:5432/quark_e2e";
const ACME_TENANT_ID = 1;
const OWNER = { username: "owner@acme.test", password: "password" }; // /quark-admins -> Admin -> full
const OUTSIDER = { username: "outsider@acme.test", password: "password" }; // no group -> gate denies

let quark: ChildProcess | undefined;

async function reachable(url: string): Promise<boolean> {
  try {
    return (await fetch(url)).ok;
  } catch {
    return false;
  }
}

async function waitFor(url: string, label: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await reachable(url)) return;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`${label} not reachable at ${url} within ${timeoutMs}ms`);
}

function newestBinary(): string {
  const bin = [
    "../target/release/quark.exe",
    "../target/debug/quark.exe",
    "../target/release/quark",
    "../target/debug/quark",
  ]
    .map((p) => resolve(p))
    .filter(existsSync)
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!bin) throw new Error("quark binary not found — build it: cargo build");
  return bin;
}

test.beforeAll(async () => {
  // 0) The acme realm must be imported (docker compose -f docker-compose.e2e.yml up -d
  // mounts e2e/keycloak/, which now includes acme-realm.json).
  await waitFor(`${KEYCLOAK}/realms/acme/.well-known/openid-configuration`, "acme realm", 90_000);

  // 1) Fresh cloud DB (idempotent — CREATE fails harmlessly if it exists).
  try {
    execSync(`docker exec quark-postgres-1 psql -U quark -d quark -c "CREATE DATABASE quark_e2e"`, {
      stdio: "ignore",
    });
  } catch {
    /* already exists */
  }

  // 2) Start a CLOUD quark (multi-tenant + Postgres) on :8082. A global OIDC is
  // set only so `oidc_configured` is true (admin_guard honours the session
  // cookie); the actual login uses the tenant's own acme config, not this.
  const bin = newestBinary();
  const signingKey = Buffer.alloc(32, 7).toString("base64");
  const log = openSync(resolve("./e2e/.quark-e2e-cloud.log"), "w");
  quark = spawn(bin, {
    env: {
      ...process.env,
      QUARK_ADDR: "127.0.0.1:8082",
      QUARK_MULTI_TENANT: "1",
      QUARK_DATABASE_URL: DB,
      QUARK_KEY: "12345678901234567",
      QUARK_SIGNING_KEY: signingKey,
      QUARK_ADMIN_TOKEN: "dev-admin-token",
      QUARK_OIDC_ISSUER: "http://localhost:8081/realms/quark",
      QUARK_OIDC_CLIENT_ID: "quark",
      QUARK_OIDC_CLIENT_SECRET: "quark-e2e-secret",
      QUARK_OIDC_REDIRECT_URL: "http://localhost:8082/admin/callback",
      QUARK_OIDC_ADMIN_CLAIM: "groups",
      QUARK_OIDC_ADMIN_VALUE: "quark-admins",
      QUARK_OIDC_READONLY_VALUE: "quark-readers",
    },
    stdio: ["ignore", log, log],
    windowsHide: true,
  });
  await waitFor(`${CLOUD_API}/admin/me`, "cloud quark :8082");

  // 3) Seed the acme tenant + its per-tenant OIDC config (issuer -> acme realm).
  // `tenants` has no RLS; `oidc_configs` is NOT_FORCED (owner bypasses), so a
  // direct upsert as the table owner works. Idempotent.
  const seed = `
    INSERT INTO tenants (id, name, slug, created) VALUES (${ACME_TENANT_ID}, 'Acme', 'acme', 0)
      ON CONFLICT (id) DO NOTHING;
    INSERT INTO oidc_configs (id, tenant_id, issuer, blob, created) VALUES (
      1, ${ACME_TENANT_ID}, 'http://localhost:8081/realms/acme',
      '{"client_id":"quark","client_secret":"acme-e2e-secret","scopes":["openid","profile","email"],"admin_claim":"groups","admin_value":"quark-admins","readonly_value":"quark-readers","required_value":"quark-readers","post_login_url":null}'::jsonb,
      0
    ) ON CONFLICT (tenant_id) DO UPDATE SET issuer = EXCLUDED.issuer, blob = EXCLUDED.blob;`;
  execSync(`docker exec -i quark-postgres-1 psql -U quark -d quark_e2e`, {
    input: seed,
    stdio: ["pipe", "ignore", "inherit"],
  });
});

test.afterAll(async () => {
  if (quark?.pid) {
    try {
      if (process.platform === "win32") execSync(`taskkill /PID ${quark.pid} /F /T`, { stdio: "ignore" });
      else quark.kill("SIGKILL");
    } catch {
      /* already gone */
    }
  }
});

async function loginViaOrg(page: Page, org: string, user: { username: string; password: string }) {
  await page.goto(`${CLOUD_API}/admin/login?org=${org}`);
  await page.waitForURL(new RegExp(`localhost:8081/realms/${org}/protocol/openid-connect`));
  await page.fill("#username", user.username);
  await page.fill("#password", user.password);
  await page.click("#kc-login");
}

test("owner in quark-admins logs in via ?org=acme and gets a full-scope session in the acme tenant", async ({ page }) => {
  await loginViaOrg(page, "acme", OWNER);
  // Success callback (post_login_url null) redirects to "/" on the cloud API.
  await page.waitForURL(/localhost:8082\//, { timeout: 15_000 });

  const me = await page.request.get(`${CLOUD_API}/admin/me`);
  const body = await me.json();
  expect(body.authenticated).toBe(true);
  expect(body.display).toBe(OWNER.username);
  // quark-admins -> Admin -> full scope; membership born in the acme tenant.
  expect(body.scopes).toContain("full");
  expect(body.current_tenant).toBe(ACME_TENANT_ID);
});

test("outsider with no group is denied by the default-closed required-group gate", async ({ page }) => {
  await loginViaOrg(page, "acme", OUTSIDER);
  // The callback returns 403 before any session is created; no session cookie.
  const me = await page.request.get(`${CLOUD_API}/admin/me`);
  const body = await me.json();
  expect(body.authenticated).toBe(false);
});
