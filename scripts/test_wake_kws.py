"""KWS session, identity and isolation regression tests; no microphone needed."""
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources'))
from wake_kws import WakeKws, validate_config


class FakeStream:
    def accept_waveform(self, rate, samples): pass
    def input_finished(self): pass


class FakeModel:
    def __init__(self): self.ready = True; self.hit = True
    def create_stream(self): return FakeStream()
    def is_ready(self, stream): return self.ready
    def decode_stream(self, stream): self.ready = False
    def get_result(self, stream): return '桃子桃子' if self.hit else ''
    def reset_stream(self, stream): pass
    def timestamps(self, stream): return [0.1, 0.5, 0.9, 1.2]


class WakeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.verify = Mock(return_value=0.8)
        self.kws = WakeKws(self.tmp.name, self.tmp.name, self.verify)
        self.model = FakeModel()
        self.kws._model = Mock(return_value=self.model)
        (Path(self.tmp.name) / 'Me.bin').write_bytes(b'test')

    def config(self, **kwargs):
        return dict(phrase='桃子桃子', owner_only=True, speaker='Me', **kwargs)

    def test_selected_owner_is_verified(self):
        sid = self.kws.create(self.config())
        r = self.kws.feed(sid, np.ones(16000), 0)
        self.assertTrue(r['speaker_match'])
        self.assertEqual(self.verify.call_args.args[1], 'Me')

    def test_other_voice_is_rejected(self):
        self.verify.return_value = 0.2
        sid = self.kws.create(self.config())
        r = self.kws.feed(sid, np.ones(16000), 0)
        self.assertTrue(r['keyword_match']); self.assertFalse(r['speaker_match'])

    def test_anyone_mode_skips_identity(self):
        sid = self.kws.create(dict(phrase='star start', owner_only=False))
        self.assertTrue(self.kws.feed(sid, np.ones(3200), 0)['speaker_match'])
        self.verify.assert_not_called()

    def test_no_keyword_cannot_wake(self):
        self.model.hit = False
        sid = self.kws.create(self.config())
        self.assertFalse(self.kws.feed(sid, np.ones(3200), 0)['keyword_match'])
        self.verify.assert_not_called()

    def test_missing_voiceprint_fails_closed(self):
        with self.assertRaises(ValueError):
            self.kws.create(dict(phrase='桃子桃子', owner_only=True, speaker='Other'))

    def test_out_of_order_audio_invalidates_session(self):
        sid = self.kws.create(self.config())
        with self.assertRaises(ValueError): self.kws.feed(sid, np.ones(3200), 2)
        self.assertNotIn(sid, self.kws.sessions)

    def test_closed_session_cannot_trigger(self):
        sid = self.kws.create(self.config()); self.kws.close(sid)
        with self.assertRaises(ValueError): self.kws.feed(sid, np.ones(3200), 0)

    def test_cooldown_does_not_retrigger(self):
        sid = self.kws.create(self.config())
        self.kws.feed(sid, np.ones(3200), 0)
        self.model.ready = True
        self.assertFalse(self.kws.feed(sid, np.ones(3200), 1)['keyword_match'])
        self.assertEqual(self.verify.call_count, 1)

    def test_expired_session_fails_closed(self):
        sid = self.kws.create(self.config()); self.kws.sessions[sid]['time'] -= 31
        with self.assertRaises(ValueError): self.kws.feed(sid, np.ones(3200), 0)

    def test_invalid_config(self):
        for extra in [dict(phrase='x'), dict(phrase='桃子 @OTHER'), dict(phrase='桃子\n桃子'),
                      dict(kws_threshold=float('nan')), dict(owner_only='false'), dict(speaker='../Me')]:
            c = self.config(); c.update(extra)
            with self.subTest(extra=extra), self.assertRaises(ValueError): validate_config(c)

    def test_nonfinite_speaker_score_rejected(self):
        self.verify.return_value = float('nan')
        sid = self.kws.create(self.config())
        self.assertFalse(self.kws.feed(sid, np.ones(3200), 0)['speaker_match'])

    def test_sessions_are_bounded(self):
        for _ in range(4): self.kws.create(self.config())
        with self.assertRaises(ValueError): self.kws.create(self.config())


if __name__ == '__main__': unittest.main()
