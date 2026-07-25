// Local-only responsive QA sweep (LUC-96). NOT wired into CI — run by hand
// against a local dev stack:
//
//   backend:  QUARK_KEY=... QUARK_ADMIN_TOKEN=dev-admin-token QUARK_DATA=...
//             QUARK_ADDR=127.0.0.1:8080 QUARK_CORS_ORIGINS=http://localhost:5173
//             ./target/debug/quark(.exe)
//   frontend: VITE_API_BASE_URL=http://127.0.0.1:8080 npm run dev -- --port 5173 --strictPort
//   script:   node scripts/responsive-qa.mjs [--out <dir>] [--routes a,b] [--breakpoints 360x740,...]
//
// Walks every panel route across 4 breakpoints x 2 themes, screenshots each
// combination, and fails (non-zero exit + JSON violation list) if the page
// grew a horizontal scrollbar (`document.documentElement.scrollWidth` wider
// than `window.innerWidth`, with +1px tolerance). Each route check is isolated:
// a failure (nav error, or just a flaky screenshot capture) is recorded and
// the sweep moves on to the next route instead of losing the rest of that
// combo's coverage.
import { chromium } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const BREAKPOINTS = [
  { id: "360x740", width: 360, height: 740 },
  { id: "768x1024", width: 768, height: 1024 },
  { id: "1024x768", width: 1024, height: 768 },
  { id: "1440x900", width: 1440, height: 900 },
];

const THEMES = ["dark", "light"];

// Authenticated routes (login is handled separately below: it's the one
// screen this sweep visits unauthenticated). `afterGoto` runs right after
// navigation, before the settle+measure step, for routes that need one extra
// interaction (e.g. opening a dialog) rather than just rendering on load.
const AUTH_ROUTE_DEFS = [
  { id: "links", path: "/links" },
  {
    id: "links-new",
    path: "/links?new=1",
    afterGoto: (page) => page.waitForSelector('[role="dialog"]', { timeout: 5000 }).catch(() => {}),
  },
  { id: "import", path: "/import" },
  { id: "analytics", path: "/analytics" },
  { id: "pixels", path: "/pixels" },
  { id: "webhooks", path: "/webhooks" },
  { id: "extensions", path: "/extensions" },
  { id: "extensions-slack", path: "/extensions/slack" },
  { id: "tokens", path: "/tokens" },
  { id: "domains", path: "/domains" },
  { id: "app-links", path: "/app-links" },
  { id: "members", path: "/members" },
  { id: "sso-provider", path: "/sso-provider" },
  { id: "sso-domains", path: "/sso-domains" },
];

function parseArgs(argv) {
  const args = {
    out: "./responsive-qa-out",
    baseUrl: "http://localhost:5173",
    apiBase: "http://127.0.0.1:8080",
    token: "dev-admin-token",
    routeFilter: null,
    bpFilter: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    const value = argv[i + 1];
    if (flag === "--out") args.out = value;
    else if (flag === "--base-url") args.baseUrl = value;
    else if (flag === "--api-base") args.apiBase = value;
    else if (flag === "--token") args.token = value;
    else if (flag === "--routes") {
      args.routeFilter = value.split(",").map((s) => s.trim()).filter(Boolean);
    } else if (flag === "--breakpoints") {
      args.bpFilter = value.split(",").map((s) => s.trim()).filter(Boolean);
    } else {
      continue;
    }
    i++;
  }
  return args;
}

/** First seeded link's `code` (not `alias`), for the `/links/:code` stats
 * screen — the same field the panel uses to build that URL. Prefers the link
 * with alias "promo-verao", else the link with the highest click/visit count,
 * else links[0] (with a warning). Returns null (route skipped, not a hard
 * failure) if the admin API is unreachable or there is no seeded data. */
async function resolveStatsCode(apiBase, token) {
  try {
    const res = await fetch(`${apiBase}/admin/links?limit=50`, { headers: { "x-admin-token": token } });
    if (!res.ok) return null;
    const data = await res.json();
    if (!data.links || data.links.length === 0) return null;

    // Prefer the link with alias "promo-verao"
    const promoLink = data.links.find(link => link.alias === "promo-verao");
    if (promoLink) return promoLink.code;

    // Else use the link with highest click/visit count, if any exist
    const linksWithStats = data.links.filter(link => link.clicks !== undefined || link.visits !== undefined);
    if (linksWithStats.length > 0) {
      const bestLink = linksWithStats.reduce((max, current) => {
        const currentCount = (current.clicks ?? 0) + (current.visits ?? 0);
        const maxCount = (max.clicks ?? 0) + (max.visits ?? 0);
        return currentCount > maxCount ? current : max;
      });
      return bestLink.code;
    }

    // Fall back to links[0] with warning
    console.warn("resolveStatsCode: no 'promo-verao' alias or stats found, using first link");
    return data.links[0]?.code ?? null;
  } catch {
    return null;
  }
}

/** Networkidle (best-effort — a slow/absent request shouldn't wedge the
 * sweep) plus a fixed settle so CSS transitions (drawer slide-in, dialog
 * rise, both <= 0.5s) finish before we measure/screenshot. */
async function settle(page) {
  await page.waitForLoadState("networkidle", { timeout: 8000 }).catch(() => {});
  await page.waitForTimeout(500);
}

function errMessage(err) {
  return err instanceof Error ? err.message : String(err);
}

/**
 * Measures + screenshots one already-loaded route. The scrollWidth check
 * (the actual pass/fail signal) and the screenshot (a best-effort visual
 * artifact) are independent: a flaky screenshot capture — observed
 * occasionally under this host's headless Chromium, likely a compositor
 * hiccup — is retried once and, failing that, recorded in `screenshotErrors`
 * without invalidating the measurement or aborting the sweep.
 */
async function checkRoute(page, { routeId, bp, theme, out, violations, screenshotErrors }) {
  const { scrollWidth, innerWidth } = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth: window.innerWidth,
  }));
  if (scrollWidth > innerWidth + 1) {
    violations.push({ route: routeId, bp: bp.id, theme, scrollWidth, innerWidth });
  }
  const shotPath = path.join(out, `${bp.id}_${theme}_${routeId}.png`);
  try {
    await page.screenshot({ path: shotPath });
  } catch {
    await page.waitForTimeout(300);
    try {
      await page.screenshot({ path: shotPath });
    } catch (err) {
      screenshotErrors.push({ route: routeId, bp: bp.id, theme, message: errMessage(err) });
    }
  }
}

/**
 * One breakpoint x theme combination: unauthenticated /login, then every
 * authenticated route. Each route visit has its own try/catch — a nav
 * failure on one route is recorded in `routeErrors` and the loop continues,
 * so a single bad route never costs the rest of the combo's coverage. Login
 * itself has no such fallback (every authed route in this combo depends on
 * it); its failure is fatal only for this combo, recorded in `comboErrors`.
 */
async function runCombo({ browser, bp, theme, authRoutes, includeLogin, args, violations, routeErrors, screenshotErrors }) {
  const context = await browser.newContext({ viewport: { width: bp.width, height: bp.height } });
  const page = await context.newPage();
  try {
    // 1. /login, unauthenticated — the one screen this sweep visits logged out.
    await page.goto(`${args.baseUrl}/login`, { waitUntil: "domcontentloaded" });
    await settle(page);
    await page.evaluate((t) => localStorage.setItem("theme", t), theme);
    await page.reload({ waitUntil: "domcontentloaded" });
    await settle(page);
    if (includeLogin) {
      try {
        await checkRoute(page, { routeId: "login", bp, theme, out: args.out, violations, screenshotErrors });
      } catch (err) {
        routeErrors.push({ route: "login", bp: bp.id, theme, message: errMessage(err) });
      }
    }

    // 2. Log in (dev break-glass admin token), then walk every authed route.
    await page.fill("#admin-token", args.token);
    await page.press("#admin-token", "Enter");
    await page.waitForURL("**/links", { timeout: 10000 });
    await settle(page);

    for (const route of authRoutes) {
      try {
        await page.goto(`${args.baseUrl}${route.path}`, { waitUntil: "domcontentloaded" });
        if (route.afterGoto) await route.afterGoto(page);
        await settle(page);
        await checkRoute(page, { routeId: route.id, bp, theme, out: args.out, violations, screenshotErrors });
      } catch (err) {
        routeErrors.push({ route: route.id, bp: bp.id, theme, message: errMessage(err) });
      }
    }
    return { ok: true };
  } catch (err) {
    return { ok: false, message: errMessage(err) };
  } finally {
    await context.close();
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  await mkdir(args.out, { recursive: true });

  const breakpoints = BREAKPOINTS.filter((bp) => !args.bpFilter || args.bpFilter.includes(bp.id));
  const includeLogin = !args.routeFilter || args.routeFilter.includes("login");

  const statsCode = await resolveStatsCode(args.apiBase, args.token);
  if (!statsCode) console.error("responsive-qa: no seeded link found, skipping the /links/:code stats route");
  const routeDefs = statsCode ? [...AUTH_ROUTE_DEFS, { id: "link-stats", path: `/links/${statsCode}` }] : AUTH_ROUTE_DEFS;
  const authRoutes = routeDefs.filter((r) => !args.routeFilter || args.routeFilter.includes(r.id));

  const browser = await chromium.launch();
  const violations = [];
  const routeErrors = [];
  const screenshotErrors = [];
  const comboErrors = [];
  let attempted = 0;

  for (const theme of THEMES) {
    for (const bp of breakpoints) {
      const before = violations.length;
      const result = await runCombo({ browser, bp, theme, authRoutes, includeLogin, args, violations, routeErrors, screenshotErrors });
      attempted += (includeLogin ? 1 : 0) + authRoutes.length;
      if (!result.ok) comboErrors.push({ bp: bp.id, theme, message: result.message });
      console.error(
        `responsive-qa: ${bp.id} ${theme} — ${violations.length - before} violation(s)${result.ok ? "" : ` (combo error: ${result.message})`}`,
      );
    }
  }

  await browser.close();

  // Screenshot-only flakes don't invalidate the scrollWidth measurement they
  // came with, so they're reported but don't fail the sweep on their own;
  // violations, route errors (couldn't measure at all), and combo errors
  // (login itself failed, so a whole combo's authed routes were skipped) do.
  const ok = violations.length === 0 && routeErrors.length === 0 && comboErrors.length === 0;
  const summary = {
    ok,
    attempted,
    violations: violations.length,
    violationDetails: violations,
    routeErrors,
    comboErrors,
    screenshotErrors,
  };
  const json = JSON.stringify(summary, null, 2);
  await writeFile(path.join(args.out, "summary.json"), json);
  console.log(json);

  process.exitCode = ok ? 0 : 1;
}

await main();
