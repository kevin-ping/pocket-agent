#!/usr/bin/env python3
"""Pretend to be the phone on PA's /audio socket (FEAT-DEV1).

Streams a WAV file as the phone microphone (PCM16LE 16 kHz mono, real time,
silence before and after), saves every sentence PA sends back as an mp3,
and reports playing / done the way the phone does. Connects to the PA that
is already running — never starts its own stt-server.

    scripts/phone_sim.py hello.wav                  # wake phrase + a question
    scripts/phone_sim.py hello.wav --barge-in stop.wav --barge-in-after 1
    scripts/phone_sim.py --camera test.jpg --idle 600  # camera open, answers snapshots

A paired phone's presence on /phone (FEAT-MP3), one link per PA, with the
same occupancy rules as the Android app (AudioService): the phone alone
decides who uses it, refuses everyone else while one PA occupies it, and is
freed by `release`, by "disconnect", or by the occupant offline for --lease s:

    scripts/phone_sim.py --pa 127.0.0.1:18650 --pa 127.0.0.1:18660 --scenario --lease 5

--scenario drives both PAs through switch_device (MCP, as Hermes would) and
checks: A enters; B is refused while A uses the phone; A parks → B enters;
B offline within the lease → A refused, after it → A enters. Exit code 1 on
any failed check. Without --scenario the links just stay up (Ctrl-C to quit).

Key: --key, else $API_SERVER_KEY, else ~/.pocket-agent/.env.
Needs `websockets` (present in ~/.pocket-agent/venv).
"""

import argparse
import asyncio
import audioop
import base64
import json
import os
import pathlib
import time
import urllib.parse
import urllib.request
import uuid
import wave

import websockets

RATE = 16000
FRAME_MS = 20
FRAME_BYTES = RATE * 2 * FRAME_MS // 1000
# edge-tts mp3: 24 kHz mono at 48 kbit/s — good enough to pace fake playback.
MP3_BYTES_PER_SEC = 48000 / 8


def load_key(cli_key):
    if cli_key:
        return cli_key
    if os.environ.get("API_SERVER_KEY"):
        return os.environ["API_SERVER_KEY"]
    env = pathlib.Path.home() / ".pocket-agent" / ".env"
    for line in env.read_text().splitlines() if env.exists() else []:
        if line.startswith("API_SERVER_KEY="):
            return line.split("=", 1)[1].strip().strip('"').strip("'")
    raise SystemExit("no API_SERVER_KEY (use --key)")


def load_pcm(path):
    """Any 16-bit WAV → 16 kHz mono PCM16LE."""
    with wave.open(str(path), "rb") as w:
        if w.getsampwidth() != 2:
            raise SystemExit(f"{path}: need 16-bit PCM")
        data = w.readframes(w.getnframes())
        if w.getnchannels() == 2:
            data = audioop.tomono(data, 2, 0.5, 0.5)
        elif w.getnchannels() != 1:
            raise SystemExit(f"{path}: need mono or stereo")
        if w.getframerate() != RATE:
            data, _ = audioop.ratecv(data, 2, 1, w.getframerate(), RATE, None)
    return data


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


class Phone:
    def __init__(self, ws, out_dir, camera=None):
        self.ws = ws
        self.camera = camera  # JPEG bytes returned for every snapshot request
        self.out_dir = out_dir
        self.queue = asyncio.Queue()
        self.saved = []
        self.last_activity = time.monotonic()
        self.played = asyncio.Event()  # first sentence started
        self.player_task = None
        self.mic_buf = bytearray()

    def say(self, pcm, label):
        log(f"mic: sending {label} ({len(pcm) / 2 / RATE:.1f}s)")
        self.mic_buf += pcm
        # Idle counts from when this speech has finished playing out.
        self.last_activity = max(self.last_activity, time.monotonic()) + len(pcm) / 2 / RATE

    async def mic(self):
        """The phone streams continuously: queued speech, else silence, in real time."""
        start = time.monotonic()
        sent = 0
        while True:
            frame = bytes(self.mic_buf[:FRAME_BYTES]).ljust(FRAME_BYTES, b"\x00")
            del self.mic_buf[:FRAME_BYTES]
            await self.ws.send(frame)
            sent += 1
            delay = start + sent * FRAME_MS / 1000 - time.monotonic()
            if delay > 0:
                await asyncio.sleep(delay)

    async def player(self):
        while True:
            seq, audio = await self.queue.get()
            await self.ws.send(json.dumps({"type": "playing", "seq": seq}))
            self.played.set()
            secs = len(audio) / MP3_BYTES_PER_SEC
            log(f"play: seq={seq} {len(audio)} bytes (~{secs:.1f}s)")
            await asyncio.sleep(secs)
            await self.ws.send(json.dumps({"type": "done", "seq": seq}))
            self.last_activity = time.monotonic()

    def clear(self):
        while not self.queue.empty():
            self.queue.get_nowait()
        if self.player_task:
            self.player_task.cancel()
        self.player_task = asyncio.ensure_future(self.player())

    async def receive(self):
        self.player_task = asyncio.ensure_future(self.player())
        expect_audio = None
        async for msg in self.ws:
            self.last_activity = time.monotonic()
            if isinstance(msg, bytes):
                if expect_audio is None:
                    log(f"unexpected binary frame ({len(msg)} bytes)")
                    continue
                path = self.out_dir / f"{len(self.saved) + 1:02d}-seq{expect_audio}.mp3"
                path.write_bytes(msg)
                self.saved.append(path)
                await self.queue.put((expect_audio, msg))
                expect_audio = None
                continue
            data = json.loads(msg)
            if data.get("type") == "play":
                expect_audio = data["seq"]
            elif data.get("type") == "clear":
                log("clear: dropping queued audio, stopping playback")
                self.clear()
            elif data.get("type") == "snapshot":
                log(f"snapshot: id={data['id']} → {'jpeg' if self.camera else 'off'}")
                reply = {"type": "snapshot", "id": data["id"]}
                if self.camera:
                    reply["jpeg"] = base64.b64encode(self.camera).decode()
                await self.ws.send(json.dumps(reply))
            else:
                log(f"control: {data}")


# ── /phone: presence and occupancy (FEAT-MP3) ─────────────────────────────────


class PaLink:
    """One PA as the phone sees it: its /phone link, and /ui + /audio while it is the occupant."""

    def __init__(self, host):
        self.host = host
        self.ws = None
        self.online = False
        self.offline = False  # the scenario cut this PA off (its computer "asleep")
        self.lost_at = time.monotonic()
        self.pa_id = ""
        self.pa_name = host
        self.ui = None
        self.audio = None


class MultiPhone:
    def __init__(self, hosts, key, lease):
        self.links = [PaLink(h) for h in hosts]
        self.key = key
        self.lease = lease
        self.occupant = None  # a PaLink
        self.phone_id = f"sim-{uuid.uuid4().hex[:8]}"
        self.events = asyncio.Queue()  # what happened, for the scenario
        self.backlog = []

    def state(self, kind="state"):
        o = self.occupant
        return {
            "type": kind,
            "foreground": True,
            "occupant": {"pa_id": o.pa_id, "pa_name": o.pa_name} if o else None,
        }

    async def broadcast(self):
        for link in self.links:
            if link.ws is not None:
                try:
                    await link.ws.send(json.dumps(self.state()))
                except websockets.ConnectionClosed:
                    pass

    async def run_link(self, link):
        url = f"ws://{link.host}/phone?token={urllib.parse.quote(self.key)}"
        while True:
            if link.offline:
                await asyncio.sleep(0.2)
                continue
            try:
                async with websockets.connect(url) as ws:
                    link.ws = ws
                    hello = self.state("hello") | {"phone_id": self.phone_id, "phone_name": "phone_sim"}
                    await ws.send(json.dumps(hello))
                    link.online = True
                    async for msg in ws:
                        await self.on_message(link, json.loads(msg))
                        if link.offline:
                            break
            except (OSError, websockets.WebSocketException) as e:
                if link.online:
                    log(f"{link.pa_name}: /phone lost ({e})")
            if link.online:
                link.lost_at = time.monotonic()
            link.ws = None
            link.online = False
            await asyncio.sleep(1)

    async def on_message(self, link, msg):
        kind = msg.get("type")
        if kind == "welcome":
            link.pa_id, link.pa_name = msg["pa_id"], msg.get("pa_name") or link.host
            log(f"{link.host}: welcome from {link.pa_name}")
            await self.events.put(("online", link))
        elif kind == "enter_request":
            reply = {"type": "enter_reply", "req_id": msg["req_id"]}
            if self.occupant is not None and self.occupant is not link:
                reply |= {"ok": False, "reason": "busy", "busy_with": self.occupant.pa_name}
            else:
                reply["ok"] = True
            log(f"{link.pa_name}: enter_request → {'ok' if reply['ok'] else 'busy'}")
            await link.ws.send(json.dumps(reply))
            if reply["ok"]:
                await self.enter(link)
        elif kind == "release":
            if self.occupant is link:
                log(f"{link.pa_name}: release")
                await self.leave()
        else:
            log(f"{link.pa_name}: {msg}")

    async def enter(self, link):
        if self.occupant is link:
            return
        self.occupant = link
        await self.broadcast()
        token = urllib.parse.quote(self.key)
        link.ui = await websockets.connect(f"ws://{link.host}/ui/ws?token={token}")
        await link.ui.send(json.dumps({"id": 1, "cmd": "set_ui_visible", "args": {"visible": True}}))
        link.audio = await websockets.connect(f"ws://{link.host}/audio?token={token}")
        log(f"now used by {link.pa_name} (/ui + /audio open)")
        await self.events.put(("entered", link))

    async def leave(self):
        link, self.occupant = self.occupant, None
        if link is None:
            return
        for ws in (link.ui, link.audio):
            if ws is not None:
                await ws.close()
        link.ui = link.audio = None
        await self.broadcast()
        log(f"free again (left {link.pa_name})")
        await self.events.put(("left", link))

    async def park(self, link):
        """The page's 「移到电脑」 button, on the occupant's /ui."""
        await link.ui.send(json.dumps({"id": 2, "cmd": "park", "args": {}}))

    async def lease_watch(self):
        while True:
            await asyncio.sleep(0.2)
            o = self.occupant
            if o is not None and not o.online and time.monotonic() - o.lost_at >= self.lease:
                log(f"{o.pa_name} offline for {self.lease:g}s: lease over")
                await self.leave()

    def switch(self, link, target):
        """switch_device as Hermes calls it (MCP over HTTP)."""
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": {"name": "switch_device", "arguments": {"target": target}}}).encode()
        req = urllib.request.Request(f"http://{link.host}/mcp", data=body, headers={
            "Authorization": f"Bearer {self.key}", "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.loads(r.read())["result"]["content"][0]["text"]

    async def wait_for(self, kind, link, timeout=10):
        """Events for other checks stay queued for them."""
        if (kind, link) in self.backlog:
            self.backlog.remove((kind, link))
            return True
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                event = await asyncio.wait_for(self.events.get(), deadline - time.monotonic())
            except asyncio.TimeoutError:
                break
            if event == (kind, link):
                return True
            self.backlog.append(event)
        return False


async def scenario(phone):
    a, b = phone.links[:2]
    failures = []

    def check(ok, what):
        log(("PASS " if ok else "FAIL ") + what)
        if not ok:
            failures.append(what)

    for link in (a, b):
        check(await phone.wait_for("online", link), f"{link.host} online on /phone")
    switch = lambda link, target: asyncio.to_thread(phone.switch, link, target)

    reply = await switch(a, "phone")
    log(f"A switch_device(phone): {reply}")
    check(await phone.wait_for("entered", a), "A enters (enter_request → ok, /ui + /audio)")

    reply = await switch(b, "phone")
    log(f"B switch_device(phone): {reply}")
    check(a.pa_name in reply and "使用" in reply, "B is refused while A uses the phone, naming A")

    await phone.park(a)
    check(await phone.wait_for("left", a), "A parks → release → the phone is free")
    await asyncio.sleep(0.5)  # B hears the new state
    reply = await switch(b, "phone")
    log(f"B switch_device(phone): {reply}")
    check(await phone.wait_for("entered", b), "B enters after A let go")

    # B's computer drops off the network: A is refused within the lease, enters after it.
    b.offline = True
    await b.ws.close()
    await asyncio.sleep(1)
    reply = await switch(a, "phone")
    log(f"A switch_device(phone) within the lease: {reply}")
    check(b.pa_name in reply, "A is refused while B is offline but within the lease")
    check(await phone.wait_for("left", b, timeout=phone.lease + 5), "lease over → the phone is free")
    await asyncio.sleep(0.5)
    reply = await switch(a, "phone")
    log(f"A switch_device(phone): {reply}")
    check(await phone.wait_for("entered", a), "A enters after the lease")

    await phone.park(a)
    await phone.wait_for("left", a)
    b.offline = False
    log("scenario: " + ("all passed" if not failures else f"{len(failures)} failed"))
    return not failures


async def run_phones(hosts, key, lease, run_scenario):
    phone = MultiPhone(hosts, key, lease)
    tasks = [asyncio.ensure_future(phone.run_link(l)) for l in phone.links]
    tasks.append(asyncio.ensure_future(phone.lease_watch()))
    try:
        if run_scenario:
            return await scenario(phone)
        await asyncio.gather(*tasks)
    finally:
        await phone.leave()
        for t in tasks:
            t.cancel()


async def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("wav", nargs="?", help="what the phone microphone hears (wake phrase + question)")
    ap.add_argument("--host", default="127.0.0.1:8650")
    ap.add_argument("--key")
    ap.add_argument("--out", default="phone_sim_out", help="directory for received mp3s")
    ap.add_argument("--lead", type=float, default=4.0, help="silence before the wav (s): PA re-arms wake on the phone")
    ap.add_argument("--barge-in", help="second wav, sent while a reply is playing")
    ap.add_argument("--barge-in-after", type=float, default=1.0, help="seconds into the first reply")
    ap.add_argument("--camera", help="JPEG: report the camera open and answer snapshots with it")
    ap.add_argument("--idle", type=float, default=60.0, help="quit after this long without audio from PA")
    ap.add_argument("--pa", action="append", help="/phone mode: a PA host:port (repeat for each paired PA)")
    ap.add_argument("--scenario", action="store_true", help="/phone mode: run the two-PA occupancy checks")
    ap.add_argument("--lease", type=float, default=90.0, help="/phone mode: seconds the occupant may be offline")
    args = ap.parse_args()

    if args.pa:
        if args.scenario and len(args.pa) < 2:
            raise SystemExit("--scenario needs two --pa")
        ok = await run_phones(args.pa, load_key(args.key), args.lease, args.scenario)
        raise SystemExit(0 if ok else 1)

    out_dir = pathlib.Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    key = urllib.parse.quote(load_key(args.key))
    url = f"ws://{args.host}/audio?token={key}"
    async with websockets.connect(url, max_size=None) as ws:
        log(f"connected to ws://{args.host}/audio")
        phone = Phone(ws, out_dir, pathlib.Path(args.camera).read_bytes() if args.camera else None)
        if phone.camera:
            await ws.send(json.dumps({"type": "camera", "on": True, "facing": "back", "switching": False}))
            log("camera: reported open")
        tasks = [asyncio.ensure_future(phone.mic()), asyncio.ensure_future(phone.receive())]

        async def script():
            await asyncio.sleep(args.lead)
            if not args.wav:
                return
            phone.say(load_pcm(args.wav), args.wav)
            if args.barge_in:
                await phone.played.wait()
                await asyncio.sleep(args.barge_in_after)
                phone.say(load_pcm(args.barge_in), args.barge_in)
        tasks.append(asyncio.ensure_future(script()))
        start = time.monotonic()
        while True:
            await asyncio.sleep(0.5)
            quiet = time.monotonic() - phone.last_activity
            if time.monotonic() - start > args.lead + 5 and quiet > args.idle:
                break
            if any(t.done() and t.exception() for t in tasks):
                raise next(t.exception() for t in tasks if t.done() and t.exception())
        for t in tasks:
            t.cancel()
    log(f"done: {len(phone.saved)} sentence(s) saved to {out_dir}/")
    for p in phone.saved:
        print("  ", p)


if __name__ == "__main__":
    asyncio.run(main())
