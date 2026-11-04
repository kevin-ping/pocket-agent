#!/usr/bin/env python3
"""Pretend to be the phone on PA's /audio socket (FEAT-DEV1).

Streams a WAV file as the phone microphone (PCM16LE 16 kHz mono, real time,
silence before and after), saves every sentence PA sends back as an mp3,
and reports playing / done the way the phone does. Connects to the PA that
is already running — never starts its own stt-server.

    scripts/phone_sim.py hello.wav                  # wake phrase + a question
    scripts/phone_sim.py hello.wav --barge-in stop.wav --barge-in-after 1
    scripts/phone_sim.py --camera test.jpg --idle 600  # camera open, answers snapshots

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
    args = ap.parse_args()

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
