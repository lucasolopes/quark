import { execSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { resolve } from "node:path";

const PIDFILE = resolve("./e2e/.quark.pid");

export default async function globalTeardown(): Promise<void> {
  if (!existsSync(PIDFILE)) return;
  const pid = readFileSync(PIDFILE, "utf8").trim();
  try {
    if (process.platform === "win32") execSync(`taskkill /PID ${pid} /F /T`, { stdio: "ignore" });
    else process.kill(Number(pid), "SIGTERM");
  } catch {
    // already gone
  }
  rmSync(PIDFILE, { force: true });
}
