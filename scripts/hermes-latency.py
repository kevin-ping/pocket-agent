#!/usr/bin/env python3
"""Where does a PA turn spend its time inside Hermes?

  analyze  parse Hermes agent.log: per-turn total / LLM calls / tool time / input tokens / stalls
  probe    send test prompts straight to Hermes /v1/runs (own session id) and time every event

Standard library only. Reads API_SERVER / API_SERVER_KEY from the repo .env, then ~/.pocket-agent/.env.
"""
import argparse, collections, datetime as dt, json, os, re, statistics, sys, time, urllib.request

LOG = os.path.expanduser("~/.hermes/profiles/xingli/logs/agent.log")
PROBES = {
    "screen": "截图看一下我电脑桌面现在有什么,简单说两句。",
    "code": "查一下 /private/var/www/pocket-agent 仓库最近 3 个 git 提交,简单说一下。",
    "flight": "帮我查一下明天悉尼到墨尔本的航班,说两三个就行。",
}


def p(values, q):
    values = sorted(values)
    return values[min(len(values) - 1, int(len(values) * q))] if values else 0


def parse_turns(path, since, session):
    stamp = lambda l: dt.datetime.strptime(l[:23], "%Y-%m-%d %H:%M:%S,%f")
    turns, cur = [], None
    for line in open(path, errors="ignore"):
        if not line[:4].isdigit() or line[:10] < since or session not in line:
            continue
        if "agent.turn_context: conversation turn" in line:
            m = re.search(r"history=(\d+) msg='(.{0,30})", line)
            cur = {"t0": stamp(line), "history": int(m.group(1)) if m else 0, "msg": m.group(2) if m else "",
                   "llm": [], "tokens": [], "cache": [], "tools": collections.defaultdict(float), "events": []}
        elif cur is None:
            continue
        elif "API call #" in line:
            m = re.search(r" in=(\d+) .*latency=([\d.]+)s(?: cache=(\d+)/(\d+))?", line)
            if m:
                cur["llm"].append(float(m.group(2)))
                cur["tokens"].append(int(m.group(1)))
                if m.group(3):
                    cur["cache"].append(int(m.group(3)) / max(1, int(m.group(4))))
        elif "agent.tool_executor" in line:
            m = re.search(r"[Tt]ool (\w+) (?:completed|returned error) \(([\d.]+)s", line)
            if m:
                cur["tools"][m.group(1)] += float(m.group(2))
        elif "Stream stale" in line or "Retrying API call" in line or "Stream drop" in line:
            cur["events"].append(line[11:19] + " " + line.split(": ", 1)[-1][:60].strip())
        elif "gateway.run: response ready" in line:
            cur["total"] = float(re.search(r"time=([\d.]+)s", line).group(1))
            cur["background"] = cur["msg"].startswith("Review the")
            turns.append(cur)
            cur = None
    return turns


def analyze(args):
    turns = [t for t in parse_turns(args.log, args.since, args.session) if not t["background"] and t["llm"]]
    if not turns:
        sys.exit("no turns found")
    totals = [t["total"] for t in turns]
    calls = [x for t in turns for x in t["llm"]]
    tools = collections.defaultdict(float)
    for t in turns:
        for k, v in t["tools"].items():
            tools[k] += v
    total = sum(totals)
    print(f"{len(turns)} user turns since {args.since}")
    print(f"turn      median {statistics.median(totals):.1f}s  p90 {p(totals, .9):.1f}s  max {max(totals):.0f}s")
    print(f"LLM call  median {statistics.median(calls):.1f}s  p90 {p(calls, .9):.1f}s  "
          f"calls/turn median {statistics.median(len(t['llm']) for t in turns)}  "
          f"LLM share {sum(calls) / total:.0%}")
    print(f"input     median {statistics.median(x for t in turns for x in t['tokens']) / 1000:.0f}k tokens")
    print("tool time: " + "  ".join(f"{k} {v:.0f}s" for k, v in sorted(tools.items(), key=lambda x: -x[1])[:6]))
    print(f"\nslowest {args.top}:")
    for t in sorted(turns, key=lambda t: -t["total"])[:args.top]:
        top = ",".join(f"{k}:{v:.0f}" for k, v in sorted(t["tools"].items(), key=lambda x: -x[1])[:3])
        cache = f"{min(t['cache']):.0%}" if t["cache"] else "-"
        print(f"  {t['t0']:%m-%d %H:%M} {t['total']:6.0f}s  llm {sum(t['llm']):4.0f}s/{len(t['llm']):2d} calls  "
              f"in {max(t['tokens']) // 1000:3d}k  minCache {cache:>4}  hist {t['history']:3d}  {top}  {t['msg']}")
        for e in t["events"]:
            print(f"      ! {e}")


def env():
    vals = {}
    for path in (os.path.join(os.path.dirname(__file__), "..", ".env"), os.path.expanduser("~/.pocket-agent/.env")):
        if os.path.exists(path):
            for line in open(path):
                k, _, v = line.strip().partition("=")
                if k in ("API_SERVER", "API_SERVER_KEY") and v:
                    vals[k] = v.strip().strip('"')
    return vals.get("API_SERVER", "http://localhost:8642").rstrip("/"), vals.get("API_SERVER_KEY", "")


def request(url, key, body=None, timeout=15):
    req = urllib.request.Request(url, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Content-Type": "application/json", **({"Authorization": f"Bearer {key}"} if key else {})})
    return urllib.request.urlopen(req, timeout=timeout)


def computer_hint():
    """PA's COMPUTER_HINT, so probes reach Hermes with the same instructions as a real PA turn."""
    src = open(os.path.join(os.path.dirname(__file__), "..", "src-tauri", "src", "commands", "computer.rs")).read()
    return re.search(r'COMPUTER_HINT: &str = r#"(.*?)"#', src, re.S).group(1)


def probe_one(name, text, session, base, key, deadline):
    t0 = time.monotonic()
    body = {"input": text, "instructions": computer_hint(), "session_id": session}
    run_id = json.load(request(f"{base}/v1/runs", key, body))["run_id"]
    print(f"\n=== {name}: {text}\n    run {run_id}  session {session}")
    mark = lambda: f"{time.monotonic() - t0:6.1f}s"
    first_text, tools, open_tools = None, [], {}
    stream = request(f"{base}/v1/runs/{run_id}/events", key, timeout=deadline)
    for raw in stream:
        line = raw.decode(errors="ignore").strip()
        if not line.startswith("data:"):
            continue
        ev = json.loads(line[5:])
        kind = ev.get("event", "")
        if kind == "message.delta":
            if first_text is None:
                first_text = time.monotonic() - t0
                print(f"  {mark()}  first text")
            continue
        if kind == "tool.started":
            open_tools[ev.get("tool")] = time.monotonic()
            print(f"  {mark()}  tool.started   {ev.get('tool')}")
        elif kind == "tool.completed":
            started = open_tools.pop(ev.get("tool"), None)
            took = time.monotonic() - started if started else 0
            tools.append((ev.get("tool"), took))
            print(f"  {mark()}  tool.completed {ev.get('tool')} ({took:.1f}s)")
        elif kind == "approval.request":
            # Never approve on the user's behalf: deny and keep timing.
            print(f"  {mark()}  approval.request -> deny: {ev.get('command', '')[:60]}")
            request(f"{base}/v1/runs/{run_id}/approval", key, {"request_id": ev.get("request_id"), "choice": "deny"})
        elif kind.startswith("run."):
            total = time.monotonic() - t0
            print(f"  {mark()}  {kind}")
            return {"name": name, "total": total, "first_text": first_text, "tools": tools, "status": kind}
        elif kind not in ("reasoning.available",):
            print(f"  {mark()}  {kind}")
    return {"name": name, "total": time.monotonic() - t0, "first_text": first_text, "tools": tools, "status": "eof"}


def probe(args):
    base, key = env()
    names = list(PROBES) if args.all or not args.name else args.name
    if unknown := set(names) - set(PROBES):
        sys.exit(f"unknown probe: {', '.join(unknown)}")
    session = args.session or f"pa-latency-probe-{dt.datetime.now():%Y%m%d-%H%M%S}"
    results = [probe_one(n, PROBES[n], session if args.shared else f"{session}-{n}", base, key, args.timeout)
               for n in names]
    print("\nsummary:")
    for r in results:
        ft = f"{r['first_text']:.1f}s" if r["first_text"] is not None else "-"
        tool_s = sum(t for _, t in r["tools"])
        print(f"  {r['name']:7s} total {r['total']:6.1f}s  first text {ft:>7}  tools {len(r['tools'])} ({tool_s:.1f}s)  "
              f"other(LLM+overhead) {r['total'] - tool_s:.1f}s  {r['status']}")
    print("\nper-LLM-call detail: python3 scripts/hermes-latency.py analyze --session", session)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("analyze")
    a.add_argument("--since", default=(dt.date.today() - dt.timedelta(days=7)).isoformat())
    a.add_argument("--session", default="pocket-agent", help="substring of the session tag (default: PA sessions)")
    a.add_argument("--top", type=int, default=10)
    a.add_argument("--log", default=LOG)
    b = sub.add_parser("probe")
    b.add_argument("name", nargs="*", help=f"probe names: {', '.join(PROBES)}")
    b.add_argument("--all", action="store_true")
    b.add_argument("--session", help="session id to use (default: fresh pa-latency-probe-<time>)")
    b.add_argument("--shared", action="store_true", help="run all probes in one session instead of one each")
    b.add_argument("--timeout", type=int, default=600)
    args = ap.parse_args()
    analyze(args) if args.cmd == "analyze" else probe(args)


if __name__ == "__main__":
    main()
