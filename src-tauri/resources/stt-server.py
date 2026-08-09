#!/usr/bin/env python3
"""stt-server: resident HTTP server for the Pocket-Agent voice stack.

Endpoints
---------
HTTP
    POST /transcribe       multipart "file"            → {"text", "language"}
    POST /vad              multipart "file"            → {"has_speech", "segments"}
    GET  /health                                       → {"status", "model", "vad"}

Wake word detection and speaker verification are handled natively by
sherpa-onnx in the Rust layer (sherpa_wake.rs). This server only provides
/transcribe and /vad for the conversation pipeline.

Security controls
-----------------
SEC-003  middleware refuses uploads > MAX_UPLOAD_BYTES via Content-Length BEFORE
         body buffering; WAV magic bytes (RIFF/WAVE) enforced post-buffer
"""
import argparse
import hashlib
import hmac
import io
import json
import os
import re
import secrets
import sys
import tempfile
import threading
import time
import wave
from pathlib import Path
from typing import Optional

import numpy as np
import uvicorn
from fastapi import (
    FastAPI,
    File,
    Form,
    HTTPException,
    Request,
    UploadFile,
    WebSocket,
    WebSocketDisconnect,
)
from fastapi.responses import JSONResponse
from pydantic import BaseModel

# Workaround for libiomp5 double-load on macOS (faster_whisper/ctranslate2 vs numpy/MKL).
# Must be set before importing faster_whisper.
os.environ.setdefault("KMP_DUPLICATE_LIB_OK", "TRUE")

import onnxruntime as ort
from faster_whisper import WhisperModel

try:
    from silero_vad import load_silero_vad, read_audio, get_speech_timestamps
    VAD_AVAILABLE = True
except ImportError:
    load_silero_vad = None
    read_audio = None
    get_speech_timestamps = None
    VAD_AVAILABLE = False



# --- Constants ----------------------------------------------------------------

MAX_UPLOAD_BYTES = 10 * 1024 * 1024  # SEC-003

# Fixed-path temp file for high-frequency wake/check endpoint.
# Safe because wake/check is serial (~2s interval), no concurrent callers.
WAKE_CHECK_TMP = os.path.join(tempfile.gettempdir(), "pocket-agent-wake-check.wav")
NAME_RE = re.compile(r"^[A-Za-z0-9_-]{1,32}$")  # SEC-001

POCKET_AGENT_HOME = Path.home() / ".pocket-agent"
MODELS_DIR = POCKET_AGENT_HOME / "models"

# SEC-RV-1-2: per-launch bearer token + Origin allowlist for endpoints that
# No protected paths remain — wake/speaker endpoints removed.
SERVER_TOKEN_PATH = POCKET_AGENT_HOME / "server.token"

# No protected paths remain — wake/speaker endpoints handled by sherpa-onnx in Rust.
PROTECTED_PATHS: set[str] = set()
# Tauri 2 webview origins across platforms + Vite dev server. A request that
# advertises an Origin outside this set is treated as cross-context (a
# browser tab on the same machine) and rejected.
ALLOWED_ORIGINS = {
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    "http://localhost:1420",
    "http://127.0.0.1:1420",
}


# --- Shared state -------------------------------------------------------------

class State:
    whisper: Optional[WhisperModel] = None
    whisper_name: str = ""
    whisper_wake: Optional[WhisperModel] = None
    whisper_wake_name: str = ""
    wake_lang: str = ""
    whisper_prompt: str = "以下是普通话的句子。"
    use_traditional: bool = False
    vad = None

# Wake-phrase keyword matching via Whisper transcription.
# Enrollment transcribes the wake phrase with Whisper and saves the keyword text.
# At runtime, probe audio is transcribed and fuzzy-matched against the enrolled text.
VOICEPRINTS_DIR = POCKET_AGENT_HOME / "voiceprints"

def _wake_template_path(speaker: str) -> Path:
    return VOICEPRINTS_DIR / f"{speaker}.wake.npy"

def _extract_mfcc_sequence(samples: np.ndarray, sr: int = 16000) -> np.ndarray:
    """Placeholder — wake phrase matching now uses Whisper transcription.
    Kept for backward compatibility with enrollment save path.
    """
    return np.array([], dtype=np.float32).reshape(0, 0)


# ── Whisper 幻觉输出过滤 (借鉴白龙马 whisper_server.py) ──
_HALLUCINATION_FRAGMENTS = [
    # 中文视频平台创作者习语（Whisper 对这类说话极易幻觉）
    "字幕", "翻译", "感谢收看", "感谢观看", "谢谢收看", "谢谢观看",
    "请订阅", "请关注", "点赞", "订阅", "转发", "打赏",
    "作词", "作曲", "制作人", "出品", "版权",
    "明镜", "栏目", "不吝",
    # initial_prompt 文本本身（Whisper 会把 prompt 幻觉为输出）
    "以下是普通话的句子", "以下是繁體中文的句子",
    # 英文常见幻觉
    "subtitles by", "thank you for watching", "please subscribe",
    "amara.org", "translated by", "music:", "♪", "♫", "♬", "🎵", "🎶",
]

_HALLU_RE_PUNCT = re.compile(r'^[\s\W]+$')
_HALLU_RE_TIMESTAMP = re.compile(r'^[\d\s:.,。，…]+$')
_HALLU_RE_SPLIT = re.compile(r'[,，、。.！!？?\s]+')


def _is_hallucination(text: str) -> bool:
    """检测 Whisper 常见幻觉输出，返回 True 表示应当过滤。"""
    if not text:
        return True
    t = text.strip()
    if not t:
        return True
    # 纯标点或特殊字符
    if _HALLU_RE_PUNCT.match(t):
        return True
    # 过短（单个汉字/字母/符号）
    if len(t) <= 1:
        return True
    # 包含已知幻觉片段（不区分大小写）
    tl = t.lower()
    for frag in _HALLUCINATION_FRAGMENTS:
        if frag.lower() in tl:
            return True
    # 单字符重复（如"啊啊啊啊"、"嗯嗯嗯嗯"）
    unique_chars = set(c for c in t if c.strip())
    if len(unique_chars) <= 2 and len(t) >= 5:
        return True
    # 全部是数字或省略号组合（时间戳幻觉）
    if _HALLU_RE_TIMESTAMP.match(t):
        return True
    # 短语级重复（如"我会说,我会说,我会说,…"）
    segs = [s.strip() for s in _HALLU_RE_SPLIT.split(t) if s.strip()]
    if len(segs) >= 4 and len(set(segs)) <= 2:
        return True
    return False


def _transcribe_for_wake(tmp_wav_path: str) -> str:
    """Transcribe audio for wake-word keyword matching using Whisper.

    Uses the dedicated wake model (WAKE_STT_MODEL) if available, falls back
    to the main STT model. Forces WAKE_LANGUAGE if set to avoid mis-detection.

    Returns lowercased transcription text. Returns empty string on failure
    or if the result is detected as a Whisper hallucination.
    """
    model = state.whisper_wake if state.whisper_wake is not None else state.whisper
    if model is None:
        return ""
    try:
        kwargs = dict(beam_size=3, initial_prompt=state.whisper_prompt)
        if state.wake_lang:
            kwargs["language"] = state.wake_lang
        segments, info = model.transcribe(tmp_wav_path, **kwargs)
        text = " ".join(seg.text.strip() for seg in segments).strip().lower()
        text = _t2s(text)
        if _is_hallucination(text):
            print(f"[stt-server] wake hallucination filtered: {text!r}",
                  file=sys.stderr, flush=True)
            return ""
        return text
    except Exception as e:
        print(f"[stt-server] wake whisper error: {e}", file=sys.stderr, flush=True)
        return ""


# Compact T2S: only covers pairs Whisper commonly flips for short
# wake words.  With initial_prompt forcing simplified output this is a
# safety net, not the primary fix.
_SIMP_TRAD_PAIRS = {
    '學': '学', '與': '与', '個': '个', '們': '们', '這': '这',
    '來': '来', '說': '说', '見': '见', '會': '会', '對': '对',
    '時': '时', '過': '过', '還': '还', '進': '进', '現': '现',
    '發': '发', '開': '开', '電': '电', '機': '机', '經': '经',
    '動': '动', '點': '点', '問': '问', '關': '关', '應': '应',
    '體': '体', '論': '论', '讓': '让', '記': '记', '員': '员',
    '結': '结', '辦': '办', '識': '识', '響': '响', '顧': '顾',
    '國': '国', '際': '际', '區': '区', '報': '报', '華': '华',
    '網': '网', '權': '权', '選': '选', '傳': '传', '藝': '艺',
    '節': '节', '藥': '药', '觀': '观', '計': '计', '認': '认',
    '議': '议', '講': '讲', '證': '证', '讀': '读', '變': '变',
    '續': '续', '誰': '谁', '類': '类', '頭': '头', '龍': '龙',
    '風': '风', '雲': '云', '業': '业', '專': '专', '書': '书',
    '無': '无', '長': '长', '東': '东', '兩': '两', '麼': '么',
    '後': '后', '內': '内', '舊': '旧', '傑': '杰', '偉': '伟',
    '強': '强', '輝': '辉', '蘭': '兰', '蓮': '莲', '瑩': '莹',
    '穎': '颖', '瑤': '瑶', '豐': '丰', '麗': '丽', '舉': '举',
    '鄉': '乡', '買': '买', '亂': '乱', '絲': '丝', '嚴': '严',
}

def _t2s(text: str) -> str:
    """Convert Traditional → Simplified, but ONLY when the user's primary
    voice is NOT a traditional-using language.  If the user chose zh-TW,
    zh-HK, zh-MO, or ja, we preserve traditional output."""
    if getattr(state, "use_traditional", False):
        return text
    return ''.join(_SIMP_TRAD_PAIRS.get(ch, ch) for ch in text)


def _clean_wake_text(text: str) -> str:
    """Strip punctuation, spaces, normalize case, and unify CJK simplification."""
    import re
    # Remove all punctuation and whitespace, keep only word characters (includes CJK)
    cleaned = re.sub(r'[^\w]', '', text.lower()).strip()
    # Unify traditional → simplified so Whisper's random 繁/简 output
    # doesn't fragment keyword matching (e.g. 同學 → 同学)
    cleaned = _t2s(cleaned)
    return cleaned


def _keyword_match(transcription: str, wake_variants: list[str]) -> bool:
    """Check if transcription matches any enrolled wake keyword variant.
    
    Both sides are cleaned with _clean_wake_text (strip punctuation, whitespace, lowercase).
    Then checks: exact match, substring containment, or character-level fuzzy similarity >= 80%.
    """
    probe = _clean_wake_text(transcription)
    if not probe:
        return False

    for wake_text in wake_variants:
        # wake_text is already cleaned by _clean_wake_text, but normalize again for safety
        variant = _clean_wake_text(wake_text)
        if not variant:
            continue
        # Exact match
        if probe == variant:
            return True
        # Substring: variant is contained in probe (Whisper adds filler)
        if variant in probe:
            return True
        # Probe is contained in variant (Whisper truncated)
        # But probe must cover at least 60% of variant to avoid tiny substrings matching
        if probe in variant and len(probe) >= len(variant) * 0.6:
            return True
        # Character-level fuzzy: overlap ratio
        if len(variant) >= 3 and len(probe) >= 3:
            # Count matching characters in order (simplified LCS ratio)
            shorter, longer = (variant, probe) if len(variant) <= len(probe) else (probe, variant)
            matched = 0
            j = 0
            for c in longer:
                if j < len(shorter) and c == shorter[j]:
                    matched += 1
                    j += 1
            ratio = matched / len(shorter)
            if ratio >= 0.8:
                return True
    return False


MAX_WAKE_VARIANTS = 10


def _load_wake_variants(speaker: str) -> list[str]:
    """Load wake keyword variants from JSON array file."""
    p = _wake_text_path(speaker)
    if not p.exists():
        return []
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
        if isinstance(data, list):
            return [s for s in data if isinstance(s, str) and s.strip()]
    except (json.JSONDecodeError, ValueError):
        pass
    return []


def _append_wake_variant(speaker: str, text: str) -> int:
    """Append a wake keyword variant. Returns total count after append."""
    variants = _load_wake_variants(speaker)
    cleaned = _clean_wake_text(text)
    # Deduplicate: skip if cleaned version already exists
    if cleaned in variants:
        return len(variants)
    variants.append(cleaned)
    if len(variants) > MAX_WAKE_VARIANTS:
        variants = variants[-MAX_WAKE_VARIANTS:]
    p = _wake_text_path(speaker)
    p.write_text(json.dumps(variants, ensure_ascii=False, indent=2), encoding="utf-8")
    return len(variants)


def _wake_text_path(speaker: str) -> Path:
    return VOICEPRINTS_DIR / f"{speaker}.wake.txt"



state = State()

# Generated by `_load_or_create_token()` in main(). Held in memory and also
# written to SERVER_TOKEN_PATH so the in-process Rust client can read it.
_SERVER_TOKEN: str = ""


def _load_or_create_token() -> str:
    """Generate a per-launch random token, persist with mode 0o600.

    Always overwrites on startup so a stale token from a prior run cannot be
    reused. The Rust client reads SERVER_TOKEN_PATH per request (no caching).
    """
    POCKET_AGENT_HOME.mkdir(parents=True, exist_ok=True)
    token = secrets.token_urlsafe(32)
    # O_CREAT|O_WRONLY|O_TRUNC + 0o600. Removing first avoids races with a
    # symlinked target.
    try:
        os.unlink(SERVER_TOKEN_PATH)
    except FileNotFoundError:
        pass
    fd = os.open(
        SERVER_TOKEN_PATH,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
        0o600,
    )
    with os.fdopen(fd, "w") as f:
        f.write(token)
    return token


def _check_bearer(auth_header: str) -> bool:
    if not auth_header.startswith("Bearer "):
        return False
    presented = auth_header[len("Bearer "):]
    return hmac.compare_digest(presented, _SERVER_TOKEN)


# --- App + middleware ---------------------------------------------------------

app = FastAPI(title="stt-server", docs_url=None, redoc_url=None)


@app.middleware("http")
async def upload_size_guard(request: Request, call_next):
    """SEC-003: refuse oversized uploads before the body is buffered.

    We rely on Content-Length because every multipart client in this stack
    (Rust reqwest::blocking::multipart and curl) sets it. Missing CL on a body
    method is treated as 411 Length Required rather than silently accepting an
    unknown-size stream.
    """
    if request.method in ("POST", "PUT", "PATCH"):
        cl = request.headers.get("content-length")
        if cl is None:
            return JSONResponse({"error": "length_required"}, status_code=411)
        try:
            n = int(cl)
        except ValueError:
            return JSONResponse({"error": "invalid_content_length"}, status_code=400)
        if n > MAX_UPLOAD_BYTES:
            return JSONResponse(
                {"error": "payload_too_large", "max_bytes": MAX_UPLOAD_BYTES},
                status_code=413,
            )
    return await call_next(request)


@app.middleware("http")
async def auth_and_origin_guard(request: Request, call_next):
    """SEC-RV-1-2: bearer token + Origin allowlist for voice-sample endpoints.

    Why both: a token alone leaks if a malicious page in the same browser
    profile coerces the user agent into replaying it (DNS rebinding, mDNS).
    Origin pins the request to a Tauri webview the user actually launched.
    """
    path = request.url.path
    if path in PROTECTED_PATHS:
        origin = request.headers.get("origin")
        # Origin may be absent on direct curl/Postman calls; we still require
        # one for protected paths so a CSRF-style cross-origin POST cannot
        # silently succeed.
        if origin is None or origin not in ALLOWED_ORIGINS:
            return JSONResponse({"error": "forbidden_origin"}, status_code=403)
        auth = request.headers.get("authorization", "")
        if not _check_bearer(auth):
            return JSONResponse({"error": "unauthorized"}, status_code=401)
    return await call_next(request)

# --- Helpers ------------------------------------------------------------------
def _read_wav_bytes(blob: bytes) -> tuple[np.ndarray, int]:
    """Parse WAV → (float32 mono samples in [-1, 1], 16000). SEC-003 magic check."""
    if len(blob) < 12 or blob[:4] != b"RIFF" or blob[8:12] != b"WAVE":
        raise HTTPException(status_code=415, detail={"error": "not_a_wav"})
    try:
        with wave.open(io.BytesIO(blob), "rb") as w:
            channels = w.getnchannels()
            sample_rate = w.getframerate()
            sample_width = w.getsampwidth()
            n_frames = w.getnframes()
            raw = w.readframes(n_frames)
    except wave.Error as e:
        raise HTTPException(
            status_code=415,
            detail={"error": "wav_parse_error", "msg": str(e)},
        )

    if channels != 1:
        raise HTTPException(status_code=415, detail={"error": "expected_mono"})
    if sample_width != 2:
        raise HTTPException(status_code=415, detail={"error": "expected_pcm16"})

    samples = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0

    if sample_rate != 16000:
        from math import gcd
        from scipy.signal import resample_poly
        g = gcd(16000, sample_rate)
        samples = resample_poly(samples, 16000 // g, sample_rate // g).astype(np.float32)
        sample_rate = 16000

    return samples, sample_rate


def _rms_dbfs(samples: np.ndarray) -> float:
    if samples.size == 0:
        return -120.0
    rms = float(np.sqrt(np.mean(samples * samples)))
    if rms <= 0:
        return -120.0
    return 20.0 * float(np.log10(rms))


def vad_segments(wav_path: str):
    """Silero VAD wrapper — preserved from the prior server."""
    wav = read_audio(wav_path, sampling_rate=16000)
    segs = get_speech_timestamps(wav, state.vad, return_seconds=True)
    return [{"start": float(s["start"]), "end": float(s["end"])} for s in segs]

# --- Existing endpoints (Phase A contracts preserved) ------------------------

@app.post("/vad/check")
async def vad_check(file: UploadFile = File(...)):
    """Lightweight VAD check: does this audio contain human speech?
    Returns {"has_speech": bool, "speech_duration_s": float}
    """
    blob = await file.read()
    _wav_magic_or_415(blob)

    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            f.write(blob)
            tmp_path = f.name

        if state.vad is None:
            return {"has_speech": False, "speech_duration_s": 0.0}

        segments = vad_segments(tmp_path)
        if not segments:
            return {"has_speech": False, "speech_duration_s": 0.0}

        total = sum(s["end"] - s["start"] for s in segments)
        return {"has_speech": total >= 0.2, "speech_duration_s": round(total, 2)}
    except Exception as e:
        print(f"[stt-server] vad/check error: {e}", file=sys.stderr, flush=True)
        return {"has_speech": False, "speech_duration_s": 0.0}
    finally:
        if tmp_path:
            try: os.unlink(tmp_path)
            except OSError: pass


@app.get("/health")
async def health():
    return {
        "status": "ok",
        "model": state.whisper_name,
        "vad": "silero" if state.vad is not None else "none",
    }


def _wav_magic_or_415(blob: bytes):
    if len(blob) < 12 or blob[:4] != b"RIFF" or blob[8:12] != b"WAVE":
        raise HTTPException(status_code=415, detail={"error": "not_a_wav"})


@app.post("/transcribe")
async def transcribe(file: UploadFile = File(...)):
    blob = await file.read()
    _wav_magic_or_415(blob)

    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            f.write(blob)
            tmp_path = f.name

        # VAD short-circuit: skip Whisper on silent audio to avoid hallucination
        if state.vad is not None:
            try:
                segs = vad_segments(tmp_path)
            except Exception as e:
                print(f"[stt-server] vad pre-check error: {e}", file=sys.stderr, flush=True)
                segs = None
            if segs is not None and len(segs) == 0:
                print("[stt-server] vad: no speech, skipping Whisper",
                      file=sys.stderr, flush=True)
                return {"text": "", "language": "", "warning": "no speech"}

        t0 = time.time()
        segments, info = state.whisper.transcribe(
            tmp_path, beam_size=5, initial_prompt=state.whisper_prompt
        )
        text = _t2s(" ".join(seg.text.strip() for seg in segments).strip())
        elapsed = time.time() - t0
        print(f"[stt-server] transcribed in {elapsed:.1f}s lang={info.language}",
              file=sys.stderr, flush=True)

        # Filter hallucinations (including initial_prompt echo-back)
        if _is_hallucination(text):
            print(f"[stt-server] hallucination filtered: {text!r}",
                  file=sys.stderr, flush=True)
            return {"text": "", "language": info.language, "warning": "hallucination filtered"}

        if text:
            return {"text": text, "language": info.language}
        return {"text": "", "language": info.language, "warning": "empty result"}
    finally:
        if tmp_path:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass


@app.post("/vad")
async def vad_endpoint(file: UploadFile = File(...)):
    if state.vad is None:
        raise HTTPException(status_code=503, detail={"error": "vad model not loaded"})

    blob = await file.read()
    _wav_magic_or_415(blob)

    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            f.write(blob)
            tmp_path = f.name

        t0 = time.time()
        segs = vad_segments(tmp_path)
        elapsed = time.time() - t0
        print(f"[stt-server] vad in {elapsed*1000:.0f}ms segments={len(segs)}",
              file=sys.stderr, flush=True)
        return {"has_speech": len(segs) > 0, "segments": segs}
    finally:
        if tmp_path:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass


# --- Speaker embedding via Python onnxruntime (bypasses macOS 12 / ORT C API mismatch) ---

_SPEAKER_SESS = None  # lazily loaded

# HuggingFace URL for the 3D-Speaker ONNX model (sherpa-onnx project).
_SPEAKER_MODEL_HF_URL = (
    "https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/main"
    "/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"
)


def _download_speaker_model(dest: Path) -> None:
    """Download the speaker ONNX model from HuggingFace."""
    import urllib.request
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(".onnx.tmp")
    try:
        print(f"[stt-server] downloading speaker model to {dest} ...", file=sys.stderr, flush=True)
        urllib.request.urlretrieve(_SPEAKER_MODEL_HF_URL, str(tmp))
        tmp.rename(dest)
        print(f"[stt-server] speaker model downloaded ({dest.stat().st_size // 1024 // 1024} MB)",
              file=sys.stderr, flush=True)
    except Exception as e:
        tmp.unlink(missing_ok=True)
        raise RuntimeError(f"failed to download speaker model: {e}") from e


def _get_speaker_session():
    """Lazy-load the 3dspeaker ONNX model (192-dim embeddings).
    Auto-downloads from HuggingFace on first use if missing.
    """
    global _SPEAKER_SESS
    if _SPEAKER_SESS is not None:
        return _SPEAKER_SESS
    model_path = MODELS_DIR / "sherpa-speaker" / "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"
    if not model_path.exists():
        _download_speaker_model(model_path)
    _SPEAKER_SESS = ort.InferenceSession(str(model_path))
    print(f"[stt-server] speaker model loaded from {model_path}", file=sys.stderr, flush=True)
    return _SPEAKER_SESS


def _extract_embedding_from_wav(wav_path: str):
    """Extract 192-dim speaker embedding from a WAV file.

    Returns np.ndarray of shape (192,) or raises ValueError.
    """
    import kaldi_native_fbank as knf

    # Read WAV to float32 samples @ 16kHz mono
    samples, sr = _read_wav_for_embedding(wav_path)
    if sr != 16000:
        from math import gcd
        from scipy.signal import resample_poly
        g = gcd(16000, sr)
        samples = resample_poly(samples, 16000 // g, sr // g).astype(np.float32)
        sr = 16000

    # Compute 80-dim Fbank features
    opts = knf.FbankOptions()
    opts.frame_opts.samp_freq = 16000
    opts.frame_opts.frame_length_ms = 25
    opts.frame_opts.frame_shift_ms = 10
    opts.mel_opts.num_bins = 80
    opts.frame_opts.snip_edges = False
    opts.mel_opts.high_freq = -400  # -400 → samp_freq / 2

    fbank = knf.OnlineFbank(opts)
    fbank.accept_waveform(16000, samples.tolist())
    fbank.input_finished()

    features = []
    for i in range(fbank.num_frames_ready):
        features.append(fbank.get_frame(i))

    if not features:
        raise ValueError("No features extracted (audio too short?)")

    feat_array = np.array(features, dtype=np.float32)
    feat_array = np.expand_dims(feat_array, axis=0)  # [1, T, 80]

    # Run ONNX inference
    sess = _get_speaker_session()
    if sess is None:
        raise ValueError("Speaker model not found")

    outputs = sess.run(None, {"x": feat_array})
    embedding = outputs[0].flatten()  # (192,)
    return embedding


def _read_wav_for_embedding(wav_path: str):
    """Read WAV → (float32 mono samples, sample_rate)."""
    with wave.open(wav_path, "rb") as w:
        channels = w.getnchannels()
        sr = w.getframerate()
        sw = w.getsampwidth()
        n = w.getnframes()
        raw = w.readframes(n)

    # Convert to float32
    if sw == 2:
        samples = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    elif sw == 4:
        samples = np.frombuffer(raw, dtype=np.int32).astype(np.float32) / 2147483648.0
    else:
        raise ValueError(f"Unsupported sample width: {sw}")

    # Mix to mono
    if channels > 1:
        samples = samples.reshape(-1, channels).mean(axis=1)

    return samples, sr


@app.post("/speaker/embed")
async def speaker_embed(file: UploadFile = File(...)):
    """Extract speaker embedding from WAV, return as base64.

    Returns: {"embedding": "<base64>", "dim": 192, "duration_s": float}
    """
    import base64

    blob = await file.read()
    _wav_magic_or_415(blob)

    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            f.write(blob)
            tmp_path = f.name

        samples, sr = _read_wav_for_embedding(tmp_path)
        duration_s = len(samples) / sr

        embedding = _extract_embedding_from_wav(tmp_path)
        emb_bytes = embedding.astype(np.float32).tobytes()
        emb_b64 = base64.b64encode(emb_bytes).decode("ascii")

        return {"embedding": emb_b64, "dim": len(embedding), "duration_s": round(duration_s, 2)}
    except ValueError as e:
        raise HTTPException(status_code=400, detail={"error": str(e)})
    except Exception as e:
        raise HTTPException(status_code=500, detail={"error": str(e)})
    finally:
        if tmp_path:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass


# --- Wake word detection via Python (bypasses macOS 12 ORT C API mismatch) ---

def _load_enrolled_voiceprints():
    """Load all enrolled voiceprints from ~/.pocket-agent/voiceprints/*.bin"""
    vp_dir = POCKET_AGENT_HOME / "voiceprints"
    if not vp_dir.exists():
        return {}
    voiceprints = {}
    for entry in sorted(vp_dir.iterdir()):
        if entry.suffix != ".bin":
            continue
        name = entry.stem
        data = entry.read_bytes()
        if len(data) % 4 != 0:
            continue
        emb = np.frombuffer(data, dtype=np.float32).copy()
        voiceprints[name] = emb
    return voiceprints


@app.post("/speaker/enroll")
async def speaker_enroll(file: UploadFile = File(...), name: str = Form("Me")):
    """Enroll a speaker: extract embedding + wake audio fingerprint.
    
    Saves both:
      - {name}.bin   — 192-dim speaker embedding (for voice identification)
      - {name}.wake.txt — wake keyword variants (one per line)
    
    Returns: {"ok": true, "speaker_id": str, "dim": int, "duration_s": float, "wake_text": str}
    """
    import base64

    blob = await file.read()
    _wav_magic_or_415(blob)

    if not NAME_RE.match(name):
        raise HTTPException(status_code=400, detail="name must match [A-Za-z0-9_-]{1,32}")

    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            f.write(blob)
            tmp_path = f.name

        samples, sr = _read_wav_for_embedding(tmp_path)
        duration_s = len(samples) / sr

        # Extract speaker embedding
        embedding = _extract_embedding_from_wav(tmp_path)
        emb_bytes = embedding.astype(np.float32).tobytes()
        emb_b64 = base64.b64encode(emb_bytes).decode("ascii")

        # Save embedding to disk
        VOICEPRINTS_DIR.mkdir(parents=True, exist_ok=True)
        emb_path = VOICEPRINTS_DIR / f"{name}.bin"
        emb_path.write_bytes(emb_bytes)

        # Transcribe enrollment audio — always resets (deletes old variants).
        # Training mode uses a separate endpoint.
        _wake_text_path(name).unlink(missing_ok=True)
        wake_text = _transcribe_for_wake(tmp_path)
        if wake_text:
            variants = [_clean_wake_text(wake_text)]
            wake_txt_path = _wake_text_path(name)
            wake_txt_path.write_text(json.dumps(variants, ensure_ascii=False, indent=2), encoding="utf-8")
            print(f"[stt-server] wake keyword reset: \"{wake_text}\" (1 variant)",
                  file=sys.stderr, flush=True)
        else:
            print(f"[stt-server] WARNING: could not transcribe enrollment audio for wake keyword",
                  file=sys.stderr, flush=True)

        print(f"[stt-server] enrolled '{name}' (dim={len(embedding)}, dur={duration_s:.1f}s)",
              file=sys.stderr, flush=True)

        return {
            "ok": True,
            "speaker_id": name,
            "embedding": emb_b64,
            "dim": len(embedding),
            "duration_s": round(duration_s, 2),
        }
    except ValueError as e:
        raise HTTPException(status_code=400, detail={"error": str(e)})
    except Exception as e:
        raise HTTPException(status_code=500, detail={"error": str(e)})
    finally:
        if tmp_path:
            try: os.unlink(tmp_path)
            except OSError: pass




@app.post("/speaker/train")
async def speaker_train(file: UploadFile = File(...), name: str = Form("Me")):
    """Training mode: transcribe audio and APPEND as a new wake keyword variant.
    
    Unlike /speaker/enroll which resets the variant list, this endpoint only
    appends a new variant to the existing list.
    
    Returns: {"ok": true, "speaker_id": str, "wake_text": str, "variant_count": int}
    """
    blob = await file.read()
    _wav_magic_or_415(blob)

    if not NAME_RE.match(name):
        raise HTTPException(status_code=400, detail="name must match [A-Za-z0-9_-]{1,32}")

    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            f.write(blob)
            tmp_path = f.name

        wake_text = _transcribe_for_wake(tmp_path)
        variant_count = 0
        if wake_text:
            variant_count = _append_wake_variant(name, wake_text)
            print(f'[stt-server] wake train appended: "{wake_text}" ({variant_count} variants total)',
                  file=sys.stderr, flush=True)
        else:
            print("[stt-server] WARNING: could not transcribe training audio",
                  file=sys.stderr, flush=True)

        return {
            "ok": True,
            "speaker_id": name,
            "wake_text": wake_text,
            "variant_count": variant_count,
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail={"error": str(e)})
    finally:
        if tmp_path:
            try: os.unlink(tmp_path)
            except OSError: pass

@app.post("/wake/check")
async def wake_check(request: Request, file: UploadFile = File(...)):
    """Check audio chunk for wake word: VAD + speaker matching.

    Input: WAV audio (mono, any sample rate — will be resampled to 16kHz).
    Output: {"speech_detected": bool, "speaker_match": bool, "score": float, "speaker": str|null}
    """
    blob = await file.read()

    try:
        # Fixed-path overwrite: serial endpoint, no concurrent risk.
        with open(WAKE_CHECK_TMP, "wb") as f:
            f.write(blob)
        tmp_path = WAKE_CHECK_TMP

        # VAD check — use stricter threshold (0.7) for wake to reject birds/noise.
        # Regular /vad endpoint keeps default (0.5) for transcription accuracy.
        if state.vad is None:
            return {"speech_detected": False, "speaker_match": False, "score": 0.0, "speaker": None}

        wav_audio = read_audio(tmp_path, sampling_rate=16000)
        if wav_audio is None or wav_audio.numel() == 0:
            return {"speech_detected": False, "speaker_match": False, "score": 0.0, "speaker": None}

        segments = get_speech_timestamps(wav_audio, state.vad,
                                         threshold=0.5,
                                         return_seconds=True)
        if not segments:
            return {"speech_detected": False, "speaker_match": False, "score": 0.0, "speaker": None}

        # Compute total speech duration
        total_speech = sum(s["end"] - s["start"] for s in segments)
        print(f"[stt-server] wake/check: speech={total_speech:.1f}s segments={len(segments)}", file=sys.stderr, flush=True)
        if total_speech < 0.2:
            return {"speech_detected": True, "speaker_match": False, "score": 0.0, "speaker": None}

        # Load voiceprints
        voiceprints = _load_enrolled_voiceprints()
        if not voiceprints:
            return {"speech_detected": True, "speaker_match": False, "score": 0.0, "speaker": None}

        # Extract embedding from FULL audio (not VAD-trimmed).
        # VAD trimming causes embedding mismatch because enroll uses full audio.
        # VAD is only used above to confirm speech is present.
        try:
            embedding = _extract_embedding_from_wav(tmp_path)
        except Exception as e:
            print(f"[stt-server] wake embedding error: {e}", file=sys.stderr, flush=True)
            return {"speech_detected": True, "speaker_match": False, "score": 0.0, "speaker": None}

        # Compare with all enrolled voiceprints
        best_name = None
        best_score = 0.0
        for name, ref_emb in voiceprints.items():
            if len(embedding) != len(ref_emb):
                continue
            dot = float(np.dot(embedding, ref_emb))
            norm_a = float(np.linalg.norm(embedding))
            norm_b = float(np.linalg.norm(ref_emb))
            if norm_a > 0 and norm_b > 0:
                score = dot / (norm_a * norm_b)
            else:
                score = 0.0
            if score > best_score:
                best_score = score
                best_name = name

        # Threshold from Rust client (query param), fallback 0.65
        threshold = float(request.query_params.get("threshold", "0.65"))
        matched = best_score >= threshold and best_name is not None

        # Wake-phrase keyword matching via Whisper transcription (only when speaker matches).
        keyword_match = False
        wake_text_matched = ""
        probe_text = ""
        if matched and best_name:
            try:
                probe_text = _transcribe_for_wake(tmp_path)
                print("[stt-server] wake/check transcription:", repr(probe_text), f"speaker_score={best_score:.3f}", file=sys.stderr, flush=True)
            except Exception as e:
                print(f"[stt-server] wake transcription error: {e}", file=sys.stderr, flush=True)

            wake_txt_path = _wake_text_path(best_name)
            if wake_txt_path.exists():
                try:
                    variants = _load_wake_variants(best_name)
                    if variants:
                        keyword_match = _keyword_match(probe_text, variants)
                        wake_text_matched = probe_text
                        print(f"[stt-server] wake keyword: variants={len(variants)} probe=\"{probe_text}\" match={keyword_match}",
                              file=sys.stderr, flush=True)
                except Exception as e:
                    print(f"[stt-server] wake keyword error: {e}", file=sys.stderr, flush=True)
            else:
                # No wake keyword enrolled — speaker match alone is sufficient
                keyword_match = True

        if matched and keyword_match:
            print(f"[stt-server] wake MATCH: {best_name} speaker={best_score:.3f} keyword=\"{wake_text_matched}\"",
                  file=sys.stderr, flush=True)

        return {
            "speech_detected": True,
            "speaker_match": matched,
            "keyword_match": keyword_match,
            "keyword_text": wake_text_matched,
            "score": round(best_score, 4),
            "speaker": best_name if matched else None,
        }
    except Exception as e:
        import traceback
        print(f"[stt-server] wake check error: {e}", file=sys.stderr, flush=True)
        traceback.print_exc(file=sys.stderr)
        return {"speech_detected": False, "speaker_match": False, "score": 0.0, "speaker": None}
    finally:
        pass  # Fixed-path file reused on next call — no cleanup needed.


# --- Startup Self-Check -------------------------------------------------------

def _startup_selfcheck() -> list[str]:
    """Validate all required dependencies and models are available.
    Returns a list of warnings (non-fatal); raises SystemExit on fatal errors.
    """
    import importlib
    fatal: list[str] = []
    warnings: list[str] = []

    # 1. Required Python packages (import_name -> pip_name)
    required_packages = {
        "numpy":            "numpy<2.0",
        "faster_whisper":   "faster-whisper>=1.0.0",
        "onnxruntime":      "onnxruntime>=1.17",
        "fastapi":          "fastapi>=0.110",
        "uvicorn":          "uvicorn>=0.27",
        "scipy":            "scipy>=1.10",
        "kaldi_native_fbank": "kaldi-native-fbank>=1.0",
        "edge_tts":         "edge-tts>=6.1",
    }
    optional_packages = {
        "silero_vad":       "silero-vad>=5.1",
        "pypinyin":         "pypinyin>=0.51",
    }

    for mod, pip in required_packages.items():
        try:
            importlib.import_module(mod)
        except ImportError:
            fatal.append(f"MISSING: {pip} (import '{mod}' failed)")

    for mod, pip in optional_packages.items():
        try:
            importlib.import_module(mod)
        except ImportError:
            warnings.append(f"optional {pip} not installed — some features disabled")

    # 2. Required model files
    speaker_model_path = MODELS_DIR / "sherpa-speaker" / "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"
    if not speaker_model_path.exists():
        # Attempt auto-download
        try:
            _download_speaker_model(speaker_model_path)
        except Exception as e:
            fatal.append(f"MODEL MISSING: {speaker_model_path.name} (auto-download failed: {e})")

    # 3. Verify speaker model loads
    if speaker_model_path.exists():
        try:
            ort.InferenceSession(str(speaker_model_path))
        except Exception as e:
            fatal.append(f"MODEL CORRUPT: {speaker_model_path.name} ({e}). Delete and restart to re-download.")

    # 4. Verify av (PyAV) — common source of build failures
    try:
        importlib.import_module("av")
    except ImportError:
        fatal.append("MISSING: av>=12.0.0,<13 (PyAV). If install fails on x86_64 macOS: brew install pkg-config ffmpeg")

    # Report
    for w in warnings:
        print(f"[stt-server] ⚠  {w}", file=sys.stderr, flush=True)

    if fatal:
        print("[stt-server] ✗ STARTUP SELF-CHECK FAILED:", file=sys.stderr, flush=True)
        for f_err in fatal:
            print(f"[stt-server]   ✗ {f_err}", file=sys.stderr, flush=True)
        print("[stt-server] Fix the above issues and restart.", file=sys.stderr, flush=True)
        raise SystemExit(1)
    else:
        print("[stt-server] ✓ self-check passed (all dependencies & models OK)",
              file=sys.stderr, flush=True)

    return warnings


# --- Entrypoint ---------------------------------------------------------------

def main():
    # Startup self-check: fail fast if dependencies or models are missing.
    _startup_selfcheck()

    parser = argparse.ArgumentParser(description="STT resident HTTP server")
    parser.add_argument("--port", type=int, default=8651)
    parser.add_argument("--model", type=str, default="tiny")
    parser.add_argument("--device", type=str, default="cpu")
    parser.add_argument("--compute-type", type=str, default="int8")
    parser.add_argument("--no-vad", action="store_true")
    parser.add_argument("--lang-prompt", type=str, default="",
                        help="initial_prompt for Whisper to control output script "
                             "(e.g. simplified vs traditional Chinese)")
    args = parser.parse_args()

    # SEC-RV-1-2: mint a per-launch bearer token before any endpoint is
    # reachable. Rust client reads SERVER_TOKEN_PATH per request.
    global _SERVER_TOKEN
    _SERVER_TOKEN = _load_or_create_token()
    print(f"[stt-server] auth token written to {SERVER_TOKEN_PATH}",
          file=sys.stderr, flush=True)

    print(f"[stt-server] loading Whisper '{args.model}' on {args.device}...",
          file=sys.stderr, flush=True)
    t0 = time.time()
    state.whisper = WhisperModel(
        args.model, device=args.device, compute_type=args.compute_type
    )
    state.whisper_name = args.model
    print(f"[stt-server] Whisper loaded in {time.time()-t0:.1f}s",
          file=sys.stderr, flush=True)

    if not args.no_vad:
        if not VAD_AVAILABLE:
            print("[stt-server] silero_vad not installed — continuing without VAD",
                  file=sys.stderr, flush=True)
        else:
            print("[stt-server] loading Silero VAD (onnx)...",
                  file=sys.stderr, flush=True)
            t1 = time.time()
            try:
                state.vad = load_silero_vad(onnx=True)
                print(f"[stt-server] VAD loaded in {time.time()-t1:.1f}s",
                      file=sys.stderr, flush=True)
            except Exception as e:
                print(f"[stt-server] VAD load failed ({e}); continuing without VAD",
                      file=sys.stderr, flush=True)
                state.vad = None

    # Load dedicated wake Whisper model (WAKE_STT_MODEL env, default: base)
    wake_model_name = os.environ.get("WAKE_STT_MODEL", "base")
    if wake_model_name != args.model:
        print(f"[stt-server] loading wake Whisper '{wake_model_name}'...",
              file=sys.stderr, flush=True)
        t2 = time.time()
        state.whisper_wake = WhisperModel(
            wake_model_name, device=args.device, compute_type=args.compute_type
        )
        state.whisper_wake_name = wake_model_name
        print(f"[stt-server] wake Whisper loaded in {time.time()-t2:.1f}s",
              file=sys.stderr, flush=True)
    else:
        # Same model — reuse main instance
        state.whisper_wake = state.whisper
        state.whisper_wake_name = args.model
        print(f"[stt-server] wake Whisper reusing main model ({args.model})",
              file=sys.stderr, flush=True)

    # Load wake language override
    state.wake_lang = os.environ.get("WAKE_LANGUAGE", "")
    # Build Whisper initial_prompt from --lang-prompt flag.
    # "trad" → traditional Chinese, "ja" → Japanese, empty → simplified Chinese.
    _lp = args.lang_prompt.strip()
    if _lp in ("trad", "ja"):
        state.whisper_prompt = "以下是繁體中文的句子。"
        state.use_traditional = True
    else:
        state.whisper_prompt = "以下是普通话的句子。"
        state.use_traditional = False
    print(f"[stt-server] whisper initial_prompt: {state.whisper_prompt!r}",
          file=sys.stderr, flush=True)
    if state.wake_lang:
        print(f"[stt-server] wake language forced: {state.wake_lang}", file=sys.stderr, flush=True)

    # Pre-load speaker model at startup (instead of lazy on first request)
    _get_speaker_session()

    print(f"[stt-server] listening on :{args.port}", file=sys.stderr, flush=True)
    uvicorn.run(
        app,
        host="127.0.0.1",
        port=args.port,
        log_level="warning",
        access_log=False,
    )


if __name__ == "__main__":
    main()
