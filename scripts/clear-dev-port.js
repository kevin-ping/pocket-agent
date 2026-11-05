// Check port 1420 is free before vite starts. Called by npm run dev.
// Nothing is ever stopped: any process on the port is reported and npm run dev
// stops, so the user decides what to end. vite's strictPort alone misses an
// IPv4-only listener, since vite binds localhost as [::1]. When the check
// itself cannot run, that is reported too: unknown is not free.
import { spawnSync } from "child_process";
import { realpathSync } from "fs";
import { fileURLToPath } from "url";

const PORT = 1420;

// System paths first: a PATH without /usr/sbin must not turn into "port free".
const TOOLS = {
  lsof: ["/usr/sbin/lsof", "lsof"],
  ps: ["/bin/ps", "ps"],
  netstat: ["netstat"],
};

/** Run the first available copy of `tool`; `error` is set only when none could run. */
export function runTool(tool, args) {
  let error;
  for (const cmd of TOOLS[tool]) {
    const r = spawnSync(cmd, args, { encoding: "utf8" });
    if (r.error?.code === "ENOENT") { error = r.error; continue; }
    return { status: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "", error: r.error };
  }
  return { status: null, stdout: "", stderr: "", error };
}

/**
 * Who listens on `port`. Returns { state: "free" } | { state: "busy", listeners:
 * [{ pid, command }] } | { state: "unknown", reason }.
 */
export function checkPort(port, platform = process.platform, run = runTool) {
  if (platform === "win32") {
    const r = run("netstat", ["-aon"]);
    if (r.error || r.status !== 0) return { state: "unknown", reason: `netstat failed: ${r.error?.message || r.stderr.trim() || `exit ${r.status}`}` };
    // The state column is localized (LISTENING, 侦听, ABHÖREN, ...), so a TCP
    // listener is told by its foreign address, always 0.0.0.0:0 or [::]:0.
    const listeners = r.stdout.split(/\r?\n/)
      .map((line) => line.trim().split(/\s+/))
      .filter((cols) => cols.length === 5 && cols[0].startsWith("TCP") && cols[1].endsWith(`:${port}`)
        && (cols[2] === "0.0.0.0:0" || cols[2] === "[::]:0"))
      .map((cols) => ({ pid: cols[4], command: "" }));
    return listeners.length ? { state: "busy", listeners } : { state: "free" };
  }

  const r = run("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"]);
  if (r.error) return { state: "unknown", reason: `lsof could not run (${r.error.code ?? r.error.message})` };
  const pids = [...new Set(r.stdout.split("\n").map((s) => s.trim()).filter(Boolean))];
  const found = pids.length > 0 && pids.every((p) => /^\d+$/.test(p));
  // lsof exits 1 with no output when nothing matches. PIDs it did print are
  // reported even if it also warned; any other outcome is a failed check.
  if (!found) {
    if (r.status === 1 && pids.length === 0 && !r.stderr.trim()) return { state: "free" };
    return { state: "unknown", reason: `lsof exited ${r.status}${r.stderr.trim() ? `: ${r.stderr.trim()}` : ""}` };
  }
  const listeners = pids.map((pid) => {
    const ps = run("ps", ["-o", "command=", "-p", pid]);
    return { pid, command: ps.error || ps.status !== 0 ? "" : ps.stdout.trim() };
  });
  return { state: "busy", listeners };
}

function main() {
  const result = checkPort(PORT);
  if (result.state === "free") return;
  const win = process.platform === "win32";
  const check = win ? `netstat -aon | findstr :${PORT}` : `lsof -iTCP:${PORT} -sTCP:LISTEN`;
  if (result.state === "unknown") {
    console.error(`[dev] could not check whether port ${PORT} is free: ${result.reason}`);
    console.error(`[dev] vite was not started. Check by hand with: ${check}`);
    console.error(`[dev] if nothing is listening, start the frontend directly with: npx vite`);
    process.exit(1);
  }
  for (const { pid, command } of result.listeners) {
    console.error(`[dev] port ${PORT} is in use by PID ${pid}: ${command || "(command unavailable)"}`);
  }
  const stop = win ? "taskkill /PID <PID>" : "kill <PID>";
  console.error(`[dev] nothing was stopped. Check with: ${check}`);
  console.error(`[dev] if it is safe to end, run: ${stop}, then run npm run tauri dev again.`);
  process.exit(1);
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) main();
