"""Local keyword spotting. No transcription, no network access during detection."""
from collections import OrderedDict
from pathlib import Path
import hashlib
import math
import re
import shutil
import tarfile
import tempfile
import threading
import time
import uuid
import urllib.request

import numpy as np

MODEL_ID = 'sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20'
MODEL_URL = f'https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/{MODEL_ID}.tar.bz2'
ARCHIVE_SHA256 = '68447f4fbc67e70eee3a93961f36e81e98f47aef73ce7e7ca00885c6cd3616a6'
SUFFIX = 'epoch-13-avg-2-chunk-16-left-64'
FILES = [f'encoder-{SUFFIX}.int8.onnx', f'decoder-{SUFFIX}.onnx',
         f'joiner-{SUFFIX}.int8.onnx', 'tokens.txt', 'en.phone']


def validate_config(config):
    if not isinstance(config, dict) or not isinstance(config.get('phrase'), str):
        raise ValueError('Invalid wake configuration')
    phrase = config['phrase'].strip()
    if not 2 <= len(phrase) <= 48 or not re.fullmatch(r'[A-Za-z\u4e00-\u9fff ]+', phrase):
        raise ValueError('Use 2–48 Chinese or English characters and spaces for the wake phrase.')
    threshold = config.get('kws_threshold', 0.25)
    speaker_threshold = config.get('speaker_threshold', 0.5)
    if (not isinstance(threshold, (int, float)) or not math.isfinite(threshold)
            or not 0.05 <= threshold <= 0.9):
        raise ValueError('Invalid keyword threshold')
    if (not isinstance(speaker_threshold, (int, float)) or not math.isfinite(speaker_threshold)
            or not 0.3 <= speaker_threshold <= 0.9):
        raise ValueError('Invalid speaker threshold')
    owner_only = config.get('owner_only', True)
    if not isinstance(owner_only, bool):
        raise ValueError('Invalid owner-only setting')
    speaker = config.get('speaker', '')
    if not isinstance(speaker, str) or (owner_only and not re.fullmatch(r'[A-Za-z0-9_-]{1,32}', speaker)):
        raise ValueError('Enroll your voice before enabling owner-only wake.')
    return dict(phrase=phrase, kws_threshold=threshold, speaker_threshold=speaker_threshold,
                owner_only=owner_only, speaker=speaker)


class WakeKws:
    def __init__(self, model_root, voiceprints, verify):
        self.path = Path(model_root) / MODEL_ID
        self.voiceprints = Path(voiceprints)
        self.verify = verify
        self.lock = threading.RLock()
        self.install_lock = threading.Lock()
        self.models = OrderedDict()
        self.sessions = {}

    def status(self):
        import importlib.util
        return dict(model=MODEL_ID, ready=all((self.path / f).is_file() for f in FILES)
                    and importlib.util.find_spec('sherpa_onnx') is not None)

    def install(self):
        with self.install_lock:
            if self.status()['ready']:
                return self.status()
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(dir=self.path.parent) as tmp:
                archive = Path(tmp) / 'model.tar.bz2'
                with urllib.request.urlopen(MODEL_URL, timeout=60) as src, archive.open('wb') as dst:
                    total = 0
                    while True:
                        chunk = src.read(1024 * 1024)
                        if not chunk:
                            break
                        total += len(chunk)
                        if total > 64 * 1024 * 1024:
                            raise ValueError('Model download exceeds size limit')
                        dst.write(chunk)
                if hashlib.sha256(archive.read_bytes()).hexdigest() != ARCHIVE_SHA256:
                    raise ValueError('Model checksum mismatch; download discarded')
                staged = Path(tmp) / 'model'
                staged.mkdir()
                with tarfile.open(archive) as tar:
                    for name in FILES:
                        member = tar.getmember(f'{MODEL_ID}/{name}')
                        if not member.isfile() or member.size > 32 * 1024 * 1024:
                            raise ValueError('Invalid model archive')
                        with tar.extractfile(member) as src, (staged / name).open('wb') as dst:
                            shutil.copyfileobj(src, dst)
                self.path.mkdir(exist_ok=True)
                for name in FILES:
                    (staged / name).replace(self.path / name)
            return self.status()

    def _model(self, config):
        import sherpa_onnx
        key = (config['phrase'], config['kws_threshold'])
        if key in self.models:
            self.models.move_to_end(key)
            return self.models[key]
        if not all((self.path / f).is_file() for f in FILES):
            raise ValueError('Download the wake model in Settings first.')
        tokens = sherpa_onnx.text2token([config['phrase'].upper()], str(self.path / 'tokens.txt'),
                                      tokens_type='phone+ppinyin', lexicon=str(self.path / 'en.phone'))
        tokens = tokens[0] if tokens else []
        if not tokens:
            raise ValueError('Unable to convert this wake phrase to pronunciations')
        keyword = ' '.join(tokens) + ' @' + config['phrase'].replace(' ', '_')
        # KeywordSpotter reads the file during construction only.
        with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', encoding='utf-8') as f:
            f.write(keyword + '\n'); f.flush()
            model = sherpa_onnx.KeywordSpotter(
                tokens=str(self.path / 'tokens.txt'), encoder=str(self.path / FILES[0]),
                decoder=str(self.path / FILES[1]), joiner=str(self.path / FILES[2]),
                keywords_file=f.name, keywords_threshold=config['kws_threshold'],
                num_threads=2, provider='cpu')
        self.models[key] = model
        while len(self.models) > 2:
            self.models.popitem(last=False)
        return model

    def create(self, config):
        with self.lock:
            config = validate_config(config)
            if config['owner_only'] and not (self.voiceprints / (config['speaker'] + '.bin')).is_file():
                raise ValueError('No voiceprint for the selected speaker. Record your voice first.')
            now = time.monotonic()
            self.sessions = {k: v for k, v in self.sessions.items() if now - v['time'] < 30}
            if len(self.sessions) >= 4:
                raise ValueError('Too many wake detection sessions')
            model = self._model(config)
            sid = uuid.uuid4().hex
            self.sessions[sid] = dict(config=config, model=model, stream=model.create_stream(),
                                      audio=np.empty(0, dtype=np.float32), time=now, seq=0,
                                      cooldown=0.0)
            return sid

    def close(self, sid):
        with self.lock:
            self.sessions.pop(sid, None)

    def feed(self, sid, samples, sequence, final=False):
        with self.lock:
            s = self.sessions.get(sid)
            if s is None:
                raise ValueError('Wake session expired; restart listening')
            now = time.monotonic()
            if now - s['time'] > 30 or sequence != s['seq']:
                self.close(sid)
                raise ValueError('Wake audio sequence interrupted; restart listening')
            if len(samples) > 16000 * 10 or not np.isfinite(samples).all():
                raise ValueError('Invalid wake audio')
            s['time'] = now; s['seq'] += 1
            result = dict(keyword_match=False, speaker_match=False, score=0.0, keyword_text='')
            # Break long test/interrupt recordings into the same blocks as live input.
            for offset in range(0, len(samples), 3200):
                block = samples[offset:offset + 3200]
                if now < s['cooldown']:
                    continue
                s['audio'] = np.concatenate((s['audio'], block))[-16000 * 5:]
                s['stream'].accept_waveform(16000, block)
                result = self._decode(s)
                if result['keyword_match']:
                    break
            if final and not result['keyword_match']:
                s['stream'].accept_waveform(16000, np.zeros(8000, dtype=np.float32))
                s['stream'].input_finished()
                result = self._decode(s)
            return result

    def _decode(self, s):
        model, stream, config = s['model'], s['stream'], s['config']
        while model.is_ready(stream):
            model.decode_stream(stream)
            if model.get_result(stream):
                score = 0.0
                matched = not config['owner_only']
                if config['owner_only']:
                    timestamps = model.timestamps(stream)
                    duration = (timestamps[-1] - timestamps[0] + 0.6) if timestamps else 3.0
                    phrase_audio = s['audio'][-int(min(5.0, max(0.5, duration)) * 16000):]
                    score = float(self.verify(phrase_audio, config['speaker']))
                    matched = math.isfinite(score) and score >= config['speaker_threshold']
                model.reset_stream(stream)
                s['audio'] = np.empty(0, dtype=np.float32)
                s['cooldown'] = time.monotonic() + 1.5
                return dict(keyword_match=True, speaker_match=matched, score=score,
                            keyword_text=config['phrase'])
        return dict(keyword_match=False, speaker_match=False, score=0.0, keyword_text='')
