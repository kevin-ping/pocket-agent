// Kill any process occupying port 1420 before vite starts.
// Called by npm run dev - prevents "Port 1420 is already in use" errors.
import { execSync } from "child_process";

try {
  if (process.platform === "win32") {
    const out = execSync("netstat -aon", { encoding: "utf8" });
    for (const line of out.split("\n")) {
      if (line.includes(":1420") && line.includes("LISTENING")) {
        const pid = line.trim().split(/\s+/).pop();
        if (pid) execSync("taskkill /F /PID " + pid, { stdio: "ignore" });
      }
    }
  } else {
    execSync("lsof -ti:1420 | xargs kill -9 2>/dev/null || true", { stdio: "ignore" });
  }
} catch {
  // No process on that port - nothing to do.
}
