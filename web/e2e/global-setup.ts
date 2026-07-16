import { spawn, execSync } from "node:child_process";
import { existsSync, mkdtempSync, openSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DISCOVERY, API } from "./config";

const PIDFILE = resolve("./e2e/.quark.pid");
const LOGFILE = resolve("./e2e/.quark.log");

async function reachable(url: string): Promise<boolean> {
  try {
    const r = await fetch(url);
    return r.ok;
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
  throw new Error(`${label} did not become reachable at ${url} within ${timeoutMs}ms`);
}

function killExistingQuark(): void {
  // A stray quark from a prior run would hold :8080; clear it so this run owns
  // a fresh instance with the OIDC env below.
  try {
    if (process.platform === "win32") execSync("taskkill /IM quark.exe /F /T", { stdio: "ignore" });
    else execSync("pkill -f target/.*/quark", { stdio: "ignore" });
  } catch {
    // none running — fine
  }
}

export default async function globalSetup(): Promise<void> {
  // 1) Keycloak must already be up (docker compose -f docker-compose.e2e.yml up -d).
  if (!(await reachable(DISCOVERY))) {
    throw new Error(
      `Keycloak is not reachable at ${DISCOVERY}.\n` +
        `Start the test IdP first, from the repo root:\n` +
        `  docker compose -f docker-compose.e2e.yml up -d --wait`,
    );
  }

  // 2) Locate a built quark binary. Pick the NEWEST of debug/release by mtime so
  // a stale binary from an older build never shadows a fresh one (a release from
  // before the OIDC feature would listen but 404 on /admin/me).
  const bin = [
    "../target/release/quark.exe",
    "../target/debug/quark.exe",
    "../target/release/quark",
    "../target/debug/quark",
  ]
    .map((p) => resolve(p))
    .filter(existsSync)
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!bin) {
    throw new Error(
      `quark binary not found. Build it first from the repo root:\n  cargo build`,
    );
  }

  killExistingQuark();

  // 3) Start quark natively with OIDC pointed at the Keycloak test realm. Native
  // (not in Docker) so the browser and the backend resolve the same issuer.
  const dataDir = mkdtempSync(join(tmpdir(), "quark-e2e-"));
  const signingKey = Buffer.alloc(32, 7).toString("base64");
  const log = openSync(LOGFILE, "w");
  const child = spawn(bin, {
    env: {
      ...process.env,
      QUARK_KEY: "12345678901234567",
      QUARK_ADMIN_TOKEN: "dev-admin-token",
      QUARK_SIGNING_KEY: signingKey,
      QUARK_DATA: dataDir,
      QUARK_OIDC_ISSUER: "http://localhost:8081/realms/quark",
      QUARK_OIDC_CLIENT_ID: "quark",
      QUARK_OIDC_CLIENT_SECRET: "quark-e2e-secret",
      QUARK_OIDC_REDIRECT_URL: "http://localhost:8080/admin/callback",
      QUARK_OIDC_ADMIN_CLAIM: "groups",
      QUARK_OIDC_ADMIN_VALUE: "quark-admins",
      QUARK_OIDC_READONLY_VALUE: "quark-readers",
      QUARK_OIDC_POST_LOGIN_URL: "http://localhost:5173/",
      QUARK_CORS_ORIGINS: "http://localhost:5173",
    },
    stdio: ["ignore", log, log],
    windowsHide: true,
  });
  child.on("error", (e) => console.error(`[e2e] failed to spawn quark (${bin}):`, e));
  writeFileSync(PIDFILE, String(child.pid ?? ""));
  console.log(`[e2e] started quark pid=${child.pid} bin=${bin} data=${dataDir}`);

  // 4) Wait for quark to answer (/admin/me returns 200 even unauthenticated).
  await waitFor(`${API}/admin/me`, "quark backend");
}
