"""KWS session, identity and isolation regression tests; no microphone needed."""
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources'))
import wake_kws
from wake_kws import WakeError, WakeKws, pronounceable_text, split_word, validate_config


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


    def test_error_codes(self):
        cases = [(dict(phrase='  '), 'wake_phrase_empty'), (dict(phrase='x'), 'wake_phrase_invalid'),
                 (dict(speaker=''), 'voiceprint_missing')]
        for extra, code in cases:
            c = self.config(); c.update(extra)
            with self.subTest(code=code), self.assertRaises(WakeError) as cm: validate_config(c)
            self.assertEqual(cm.exception.code, code)
        with self.assertRaises(WakeError) as cm:
            self.kws.create(dict(phrase='桃子桃子', owner_only=True, speaker='Other'))
        self.assertEqual(cm.exception.code, 'voiceprint_missing')


class EngineTests(unittest.TestCase):
    def setUp(self):
        self.addCleanup(wake_kws._engine.clear)
        wake_kws._engine.clear()

    def fail_import(self, mac_version):
        real_import = __import__
        def fake(name, *a, **k):
            if name == 'sherpa_onnx': raise ImportError('dlopen: Symbol not found: __ZNSt3__18to_chars')
            return real_import(name, *a, **k)
        return patch('builtins.__import__', fake), patch('platform.mac_ver', return_value=(mac_version, '', ''))

    def test_old_macos_reports_versions_without_raw_error(self):
        imp, ver = self.fail_import('12.7.6')
        with imp, ver, patch('sys.stderr'):
            status = WakeKws(tempfile.gettempdir(), tempfile.gettempdir(), Mock()).status()
        self.assertFalse(status['ready'])
        self.assertEqual(status['error'], dict(code='wake_engine_unsupported_os',
                                               params=dict(current='12.7.6', required='13.4')))
        self.assertNotIn('dlopen', repr(status))

    def test_new_macos_load_failure_is_generic(self):
        imp, ver = self.fail_import('14.5')
        with imp, ver, patch('sys.stderr'):
            err = wake_kws.engine_error()
        self.assertEqual((err.code, err.status), ('wake_engine_unavailable', 503))


class SplitTests(unittest.TestCase):
    WORDS = frozenset({'HEY', 'MIMI', 'KYU', 'MI', 'KU', 'KEVIN', 'BOT', 'Z', 'ORB', 'LAX',
                       'A', 'B', 'AB', 'ABC', 'BCD', 'D'})

    def test_dictionary_words_are_kept(self):
        self.assertEqual(pronounceable_text('hey kevin', self.WORDS), 'HEY KEVIN')

    def test_unknown_word_is_split(self):
        self.assertEqual(split_word('MIMIKYU', self.WORDS), ['MIMI', 'KYU'])
        self.assertEqual(pronounceable_text('hey mimikyu', self.WORDS), 'HEY MIMI KYU')
        self.assertEqual(pronounceable_text('嘿 kevinbot', self.WORDS), '嘿 KEVIN BOT')

    def test_single_letter_pieces_are_avoided(self):
        # ABCDE+F is shorter but ends in a letter name; AB+CD+EF has none.
        words = frozenset({'ABCDE', 'F', 'AB', 'CD', 'EF'})
        self.assertEqual(split_word('ABCDEF', words), ['AB', 'CD', 'EF'])
        # With no letter-free split, the fewest pieces wins.
        self.assertEqual(split_word('ZORBLAX', self.WORDS), ['Z', 'ORB', 'LAX'])

    def test_unsplittable_word_names_the_word(self):
        self.assertIsNone(split_word('QQQ', self.WORDS))
        with self.assertRaises(WakeError) as cm: pronounceable_text('hey qqq', self.WORDS)
        self.assertEqual((cm.exception.code, cm.exception.params), ('wake_phrase_unpronounceable', dict(word='qqq')))

    def test_saved_phrase_and_label_keep_original_spelling(self):
        lexicon = Path(tempfile.mkdtemp()) / 'en.phone'
        lexicon.write_text('HEY HH EY1\nMIMI M IY1 M IY0\nKYU K Y UW1\n', encoding='utf-8')
        kws = WakeKws(tempfile.gettempdir(), tempfile.gettempdir(), Mock())
        kws.path = lexicon.parent
        seen = {}
        class FakeSherpa:
            @staticmethod
            def text2token(texts, tokens, **kw):
                seen['text'] = texts[0]; return [['HH', 'EY1']]
            class KeywordSpotter:
                def __init__(self, keywords_file, **kw): seen['keyword'] = Path(keywords_file).read_text()
        with patch.object(wake_kws, 'FILES', ['en.phone'] * 3), patch.dict(sys.modules, sherpa_onnx=FakeSherpa), \
                patch.object(wake_kws, 'engine_error', return_value=None):
            kws._model(dict(phrase='hey mimikyu', kws_threshold=0.25))
        self.assertEqual(seen['text'], 'HEY MIMI KYU')
        self.assertTrue(seen['keyword'].strip().endswith('@hey_mimikyu'))


if __name__ == '__main__': unittest.main()
