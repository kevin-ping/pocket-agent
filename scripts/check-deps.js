// Check node_modules matches package-lock.json before vite starts. Called by
// npm run dev / npm run build. After git pull a new or bumped dependency is
// otherwise only noticed when vite fails to resolve an import. Nothing is
// installed: the user is told to run npm install. When the check itself cannot
// run (no lock file), that is reported too: unknown is not up to date.
import { existsSync, readFileSync, realpathSync } from "fs";
import { join } from "path";
import { fileURLToPath } from "url";

function readJson(path) {
  try { return JSON.parse(readFileSync(path, "utf8")); } catch { return null; }
}

/**
 * Compare each direct dependency in package.json with package-lock.json and
 * node_modules. Returns { state: "ok" } | { state: "stale", problems: [{ name,
 * locked, installed }] } (installed is null when missing) | { state: "unknown",
 * reason }.
 */
export function checkDeps(root) {
  const pkg = readJson(join(root, "package.json"));
  if (!pkg) return { state: "unknown", reason: "package.json could not be read" };
  const lock = readJson(join(root, "package-lock.json"));
  if (!lock?.packages) return { state: "unknown", reason: "package-lock.json is missing or unreadable" };
  if (!existsSync(join(root, "node_modules"))) return { state: "stale", problems: [], noModules: true };

  const names = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies });
  const problems = [];
  for (const name of names) {
    const locked = lock.packages[`node_modules/${name}`]?.version ?? null;
    const installed = readJson(join(root, "node_modules", name, "package.json"))?.version ?? null;
    if (!locked) return { state: "unknown", reason: `${name} is not in package-lock.json (run npm install to update it)` };
    if (installed !== locked) problems.push({ name, locked, installed });
  }
  return problems.length ? { state: "stale", problems } : { state: "ok" };
}

function main() {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const result = checkDeps(root);
  if (result.state === "ok") return;
  if (result.state === "unknown") {
    console.error(`[deps] could not check npm dependencies: ${result.reason}`);
  } else if (result.noModules) {
    console.error("[deps] node_modules is missing.");
  } else {
    console.error("[deps] dependencies changed (common after git pull):");
    for (const { name, locked, installed } of result.problems) {
      console.error(`[deps]   ${name}: ${installed ? `installed ${installed}` : "not installed"}, package-lock.json wants ${locked}`);
    }
  }
  console.error("[deps] run npm install first, then start again. 请先运行 npm install。");
  process.exit(1);
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) main();
