#!/usr/bin/env python3
"""stt-server: Resident HTTP server for faster_whisper STT + Silero VAD.
Loads the Whisper and VAD models once at startup, then listens for requests.

Usage:
    python3 stt-server.py [--port PORT] [--model MODEL] [--device DEVICE]

Endpoints:
    POST /transcribe  — multipart form with "file" field → {"text": "...", "language": "zh"}
                        VAD runs first; if no speech, returns text:"" + warning:"no speech"
                        without invoking Whisper (prevents hallucination on silence).
    POST /vad         — multipart form with "file" field
                        → {"has_speech": bool, "segments": [{"start": sec, "end": sec}]}
    GET  /health      → {"status": "ok", "model": "tiny", "vad": "silero"}
"""
import argparse
import json
import os
import sys
import tempfile
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

# Workaround for libiomp5 double-load on macOS (faster_whisper/ctranslate2 vs numpy/MKL).
# Must be set before importing faster_whisper.
os.environ.setdefault("KMP_DUPLICATE_LIB_OK", "TRUE")

from faster_whisper import WhisperModel

try:
    from silero_vad import load_silero_vad, read_audio, get_speech_timestamps
    VAD_AVAILABLE = True
except ImportError:
    load_silero_vad = None
    read_audio = None
    get_speech_timestamps = None
    VAD_AVAILABLE = False


class SttState:
    """Shared state holding the loaded models."""
    model: WhisperModel = None
    model_name: str = ""
    vad_model = None


state = SttState()


def vad_segments(wav_path: str):
    """Run Silero VAD on a WAV file. Returns list of {start, end} in seconds."""
    wav = read_audio(wav_path, sampling_rate=16000)
    segs = get_speech_timestamps(wav, state.vad_model, return_seconds=True)
    # Normalize keys to plain floats (silero may return torch scalars)
    return [{"start": float(s["start"]), "end": float(s["end"])} for s in segs]


class SttHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        # Log to stderr so PA can capture it
        print(f"[stt-server] {fmt % args}", file=sys.stderr, flush=True)

    def do_GET(self):
        if self.path == "/health":
            self._json_response(200, {
                "status": "ok",
                "model": state.model_name,
                "vad": "silero" if state.vad_model is not None else "none",
            })
        else:
            self._json_response(404, {"error": "not found"})

    def do_POST(self):
        if self.path == "/transcribe":
            self._handle_transcribe()
        elif self.path == "/vad":
            self._handle_vad()
        else:
            self._json_response(404, {"error": "not found"})

    def _read_upload(self):
        """Parse multipart upload. Returns wav_data bytes or None."""
        content_type = self.headers.get("Content-Type", "")
        if "multipart/form-data" not in content_type:
            self._json_response(400, {"error": "expected multipart/form-data"})
            return None

        boundary = None
        for part in content_type.split(";"):
            part = part.strip()
            if part.startswith("boundary="):
                boundary = part[len("boundary="):]
                break

        if not boundary:
            self._json_response(400, {"error": "missing boundary"})
            return None

        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length)

        wav_data = self._extract_file(body, boundary.encode())
        if wav_data is None:
            self._json_response(400, {"error": "no file field found"})
            return None
        return wav_data

    def _handle_transcribe(self):
        wav_data = self._read_upload()
        if wav_data is None:
            return

        t0 = time.time()
        tmp_path = None
        try:
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
                f.write(wav_data)
                tmp_path = f.name

            # VAD short-circuit: skip Whisper on silent audio to avoid hallucination
            if state.vad_model is not None:
                try:
                    segs = vad_segments(tmp_path)
                except Exception as e:
                    print(f"[stt-server] vad pre-check error: {e}", file=sys.stderr, flush=True)
                    segs = None  # fall through to Whisper

                if segs is not None and len(segs) == 0:
                    print(f"[stt-server] vad: no speech, skipping Whisper", file=sys.stderr, flush=True)
                    self._json_response(200, {"text": "", "language": "", "warning": "no speech"})
                    return

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
            if tmp_path:
                try:
                    os.unlink(tmp_path)
                except Exception:
                    pass

    def _handle_vad(self):
        if state.vad_model is None:
            self._json_response(503, {"error": "vad model not loaded"})
            return

        wav_data = self._read_upload()
        if wav_data is None:
            return

        tmp_path = None
        try:
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
                f.write(wav_data)
                tmp_path = f.name

            t0 = time.time()
            segs = vad_segments(tmp_path)
            elapsed = time.time() - t0
            print(f"[stt-server] vad in {elapsed*1000:.0f}ms segments={len(segs)}", file=sys.stderr, flush=True)

            self._json_response(200, {
                "has_speech": len(segs) > 0,
                "segments": segs,
            })
        except Exception as e:
            print(f"[stt-server] vad error: {e}", file=sys.stderr, flush=True)
            self._json_response(500, {"error": str(e)})
        finally:
            if tmp_path:
                try:
                    os.unlink(tmp_path)
                except Exception:
                    pass

    def _extract_file(self, body: bytes, boundary: bytes):
        """Extract the first file content from multipart body."""
        parts = body.split(b"--" + boundary)
        for part in parts:
            if b"filename=" not in part:
                continue
            header_end = part.find(b"\r\n\r\n")
            if header_end == -1:
                continue
            file_data = part[header_end + 4:]
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
    parser.add_argument("--no-vad", action="store_true", help="Disable Silero VAD (do not load model)")
    args = parser.parse_args()

    print(f"[stt-server] loading Whisper '{args.model}' on {args.device}...", file=sys.stderr, flush=True)
    t0 = time.time()
    state.model = WhisperModel(args.model, device=args.device, compute_type=args.compute_type)
    state.model_name = args.model
    print(f"[stt-server] Whisper loaded in {time.time()-t0:.1f}s", file=sys.stderr, flush=True)

    if not args.no_vad:
        if not VAD_AVAILABLE:
            print(
                "[stt-server] silero_vad not installed — run: "
                "pip install silero-vad onnxruntime  (continuing without VAD)",
                file=sys.stderr, flush=True,
            )
        else:
            print(f"[stt-server] loading Silero VAD (onnx)...", file=sys.stderr, flush=True)
            t1 = time.time()
            try:
                state.vad_model = load_silero_vad(onnx=True)
                print(f"[stt-server] VAD loaded in {time.time()-t1:.1f}s", file=sys.stderr, flush=True)
            except Exception as e:
                print(f"[stt-server] VAD load failed ({e}); continuing without VAD", file=sys.stderr, flush=True)
                state.vad_model = None

    print(f"[stt-server] listening on :{args.port}", file=sys.stderr, flush=True)

    server = HTTPServer(("127.0.0.1", args.port), SttHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("[stt-server] shutting down", file=sys.stderr, flush=True)
        server.server_close()


if __name__ == "__main__":
    main()
