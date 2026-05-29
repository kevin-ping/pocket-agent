#!/usr/bin/env python3
"""stt-server: Resident HTTP server for faster_whisper STT.
Loads the Whisper model once at startup, then listens for transcribe requests.

Usage:
    python3 stt-server.py [--port PORT] [--model MODEL] [--device DEVICE]

Endpoints:
    POST /transcribe  — multipart form with "file" field → {"text": "...", "language": "zh"}
    GET  /health      → {"status": "ok", "model": "tiny"}
"""
import argparse
import json
import os
import sys
import tempfile
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

from faster_whisper import WhisperModel


class SttState:
    """Shared state holding the loaded model."""
    model: WhisperModel = None
    model_name: str = ""


state = SttState()


class SttHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        # Log to stderr so PA can capture it
        eprintln = f"[stt-server] {args[0]}" if args else f"[stt-server] {fmt}"
        print(f"[stt-server] {fmt % args}", file=sys.stderr, flush=True)

    def do_GET(self):
        if self.path == "/health":
            self._json_response(200, {"status": "ok", "model": state.model_name})
        else:
            self._json_response(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/transcribe":
            self._json_response(404, {"error": "not found"})
            return

        content_type = self.headers.get("Content-Type", "")
        if "multipart/form-data" not in content_type:
            self._json_response(400, {"error": "expected multipart/form-data"})
            return

        # Parse boundary
        boundary = None
        for part in content_type.split(";"):
            part = part.strip()
            if part.startswith("boundary="):
                boundary = part[len("boundary="):]
                break

        if not boundary:
            self._json_response(400, {"error": "missing boundary"})
            return

        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length)

        # Extract file content from multipart
        wav_data = self._extract_file(body, boundary.encode())
        if wav_data is None:
            self._json_response(400, {"error": "no file field found"})
            return

        # Write to temp file and transcribe
        t0 = time.time()
        try:
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
                f.write(wav_data)
                tmp_path = f.name

            segments, info = state.model.transcribe(tmp_path, beam_size=5)
            text = " ".join(seg.text.strip() for seg in segments).strip()
            detected_lang = info.language

            elapsed = time.time() - t0
            print(f"[stt-server] transcribed in {elapsed:.1f}s lang={detected_lang}", file=sys.stderr, flush=True)

            if text:
                self._json_response(200, {"text": text, "language": detected_lang})
            else:
                self._json_response(200, {"text": "", "language": detected_lang, "warning": "empty result"})
        except Exception as e:
            print(f"[stt-server] error: {e}", file=sys.stderr, flush=True)
            self._json_response(500, {"error": str(e)})
        finally:
            try:
                os.unlink(tmp_path)
            except Exception:
                pass

    def _extract_file(self, body: bytes, boundary: bytes):
        """Extract the first file content from multipart body."""
        # Split by boundary
        parts = body.split(b"--" + boundary)
        for part in parts:
            if b"filename=" not in part:
                continue
            # Find the double CRLF that separates headers from body
            header_end = part.find(b"\r\n\r\n")
            if header_end == -1:
                continue
            file_data = part[header_end + 4:]
            # Strip trailing \r\n
            if file_data.endswith(b"\r\n"):
                file_data = file_data[:-2]
            return file_data
        return None

    def _json_response(self, code, data):
        body = json.dumps(data, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser(description="STT resident HTTP server")
    parser.add_argument("--port", type=int, default=8651, help="Port to listen on (default: 8651)")
    parser.add_argument("--model", type=str, default="tiny", help="Whisper model size (default: tiny)")
    parser.add_argument("--device", type=str, default="cpu", help="Device: cpu or cuda (default: cpu)")
    parser.add_argument("--compute-type", type=str, default="int8", help="Compute type (default: int8)")
    args = parser.parse_args()

    print(f"[stt-server] loading model '{args.model}' on {args.device}...", file=sys.stderr, flush=True)
    t0 = time.time()
    state.model = WhisperModel(args.model, device=args.device, compute_type=args.compute_type)
    state.model_name = args.model
    print(f"[stt-server] model loaded in {time.time()-t0:.1f}s, listening on :{args.port}", file=sys.stderr, flush=True)

    server = HTTPServer(("127.0.0.1", args.port), SttHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("[stt-server] shutting down", file=sys.stderr, flush=True)
        server.server_close()


if __name__ == "__main__":
    main()
