"""KWS session, identity and isolation regression tests; no microphone needed."""
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources'))
import wake_kws
from wake_kws import (WakeError, WakeKws, effective_voiceprint, pronounceable_text, split_word,
                      validate_config, voiceprint_file)


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
        self.assertEqual(self.verify.call_args.args[1:], ('Me', 'desktop'))

    def test_voice_source_is_allowlisted_and_reaches_verify(self):
        sid = self.kws.create(self.config(voice_source='phone'))
        self.kws.feed(sid, np.ones(16000), 0)
        self.assertEqual(self.verify.call_args.args[2], 'phone')
        for bad in ['tablet', '', None, '../x']:
            with self.subTest(source=bad), self.assertRaises(ValueError):
                validate_config(self.config(voice_source=bad))

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


class VoiceprintTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def test_effective_sample_for_each_combination_and_source(self):
        desktop, phone = self.dir / 'Me.bin', self.dir / 'Me.phone.bin'
        # (desktop sample, phone sample) -> (on the desktop, on the phone)
        cases = {(False, False): (None, None), (True, False): (desktop, desktop),
                 (False, True): (None, phone), (True, True): (desktop, phone)}
        for (has_desktop, has_phone), expected in cases.items():
            for path, present in [(desktop, has_desktop), (phone, has_phone)]:
                path.unlink(missing_ok=True)
                if present:
                    path.write_bytes(b'x')
            with self.subTest(desktop=has_desktop, phone=has_phone):
                self.assertEqual((effective_voiceprint(self.dir, 'Me', 'desktop'),
                                  effective_voiceprint(self.dir, 'Me', 'phone')), expected)

    def test_source_and_name_must_be_allowlisted(self):
        self.assertEqual(voiceprint_file(self.dir, 'Me', 'phone'), self.dir / 'Me.phone.bin')
        for name, source in [('Me', 'tablet'), ('Me', '../x'), ('Me', ''), ('../Me', 'desktop'), ('Me.phone', 'desktop')]:
            with self.subTest(name=name, source=source):
                self.assertIsNone(voiceprint_file(self.dir, name, source))
        (self.dir / 'Me.phone.bin').write_bytes(b'x')
        self.assertIsNone(effective_voiceprint(self.dir, 'Me.phone', 'desktop'), 'a sample is not a speaker')

    def test_phone_sample_alone_enables_owner_wake_only_on_the_phone(self):
        (self.dir / 'Me.phone.bin').write_bytes(b'x')
        kws = WakeKws(self.dir, self.dir, Mock(return_value=0.8))
        kws._model = Mock(return_value=FakeModel())
        config = dict(phrase='桃子桃子', owner_only=True, speaker='Me')
        self.assertTrue(kws.create(dict(config, voice_source='phone')))
        with self.assertRaises(WakeError) as cm:
            kws.create(config)
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
    WORDS = frozenset({'HEY', 'MIMI', 'KYU', 'MI', 'KU', 'JARVIS', 'BOT', 'Z', 'ORB', 'LAX',
                       'A', 'B', 'AB', 'ABC', 'BCD', 'D', 'POCK', 'Y'})

    def test_dictionary_words_are_kept(self):
        self.assertEqual(pronounceable_text('hey jarvis', self.WORDS), 'HEY JARVIS')

    def test_unknown_word_is_split(self):
        self.assertEqual(split_word('MIMIKYU', self.WORDS), ['MIMI', 'KYU'])
        self.assertEqual(pronounceable_text('hey mimikyu', self.WORDS), 'HEY MIMI+KYU')
        self.assertEqual(pronounceable_text('嘿 jarvisbot', self.WORDS), '嘿 JARVIS+BOT')

    def test_split_pieces_stay_one_word(self):
        self.assertEqual(pronounceable_text('hey pocky', self.WORDS), 'HEY POCK+Y')
        # Typed apart by the user: still two words, so Y keeps its letter name.
        self.assertEqual(pronounceable_text('hey pock y', self.WORDS), 'HEY POCK Y')

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
        def text2token(text, tokens, lexicon):
            seen['text'] = text; return ['HH', 'EY1']
        class FakeSherpa:
            class KeywordSpotter:
                def __init__(self, keywords_file, **kw): seen['keyword'] = Path(keywords_file).read_text()
        with patch.object(wake_kws, 'FILES', ['en.phone'] * 3), patch.dict(sys.modules, sherpa_onnx=FakeSherpa), \
                patch.object(wake_kws, 'engine_error', return_value=None), patch.object(wake_kws, 'text2token', text2token):
            kws._model(dict(phrase='hey mimikyu', kws_threshold=0.25))
        self.assertEqual(seen['text'], 'HEY MIMI+KYU')
        self.assertTrue(seen['keyword'].strip().endswith('@hey_mimikyu'))



MODEL = Path.home() / '.pocket-agent/models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20'


class Text2TokenTests(unittest.TestCase):
    """Built-in phone+ppinyin conversion; must match sherpa-onnx 1.12.26 text2token."""
    def files(self, lexicon, tokens):
        d = Path(tempfile.mkdtemp())
        (d / 'en.phone').write_text(lexicon, encoding='utf-8')
        (d / 'tokens.txt').write_text(''.join(f'{t} {i}\n' for i, t in enumerate(tokens)), encoding='utf-8')
        return str(d / 'tokens.txt'), str(d / 'en.phone')

    def test_english_from_lexicon_and_chinese_as_partial_pinyin(self):
        tokens, lexicon = self.files('HEY HH EY1\n', ['HH', 'EY1', 'x', 'iǎo', 'īng'])
        self.assertEqual(wake_kws.text2token('HEY 小星', tokens, lexicon), ['HH', 'EY1', 'x', 'iǎo', 'x', 'īng'])

    def test_word_not_in_lexicon_is_empty(self):
        tokens, lexicon = self.files('HEY HH EY1\n', ['HH', 'EY1'])
        self.assertEqual(wake_kws.text2token('HEY QQQ', tokens, lexicon), [])

    def test_final_y_after_consonant_reads_as_suffix(self):
        tokens, lexicon = self.files('HEY HH EY1\nPOCK P AA1 K\nPA P AA1\nY W AY1\n',
                                     ['HH', 'EY1', 'P', 'AA1', 'K', 'W', 'AY1', 'IY0'])
        t2t = lambda text: ' '.join(wake_kws.text2token(text, tokens, lexicon))
        self.assertEqual(t2t('HEY POCK+Y'), 'HH EY1 P AA1 K IY0')
        self.assertEqual(t2t('HEY POCK Y'), 'HH EY1 P AA1 K W AY1')
        self.assertEqual(t2t('HEY PA+Y'), 'HH EY1 P AA1 W AY1')
        self.assertEqual(t2t('HEY POCK+QQ'), '')

    def test_token_missing_from_table_is_empty(self):
        tokens, lexicon = self.files('HEY HH EY1\n', ['HH'])
        self.assertEqual(wake_kws.text2token('HEY', tokens, lexicon), [])

    @unittest.skipUnless((MODEL / 'tokens.txt').is_file(), 'wake model not downloaded')
    def test_real_model_matches_sherpa_1_12_26(self):
        args = str(MODEL / 'tokens.txt'), str(MODEL / 'en.phone')
        self.assertEqual(' '.join(wake_kws.text2token('你好小星', *args)), 'n ǐ h ǎo x iǎo x īng')
        self.assertEqual(' '.join(wake_kws.text2token('HELLO COMPUTER', *args)), 'HH AH0 L OW1 K AH0 M P Y UW1 T ER0')
        self.assertEqual(wake_kws.text2token('ZZQXJ', *args), [])
        words = wake_kws._lexicon_words(args[1])
        self.assertEqual(' '.join(wake_kws.text2token(pronounceable_text('hey pocky', words), *args)),
                         'HH EY1 P AA1 K IY0')
        self.assertEqual(' '.join(wake_kws.text2token(pronounceable_text('hey pock y', words), *args)),
                         'HH EY1 P AA1 K W AY1')

    def test_pronounce_reports_text_and_tokens(self):
        d = Path(tempfile.mkdtemp())
        (d / 'en.phone').write_text('HEY HH EY1\nMIMI M IY1 M IY0\nKYU K Y UW1\n', encoding='utf-8')
        (d / 'tokens.txt').write_text(''.join(f'{t} {i}\n' for i, t in enumerate(
            ['HH', 'EY1', 'M', 'IY1', 'IY0', 'K', 'Y', 'UW1'])), encoding='utf-8')
        kws = WakeKws(tempfile.gettempdir(), tempfile.gettempdir(), Mock())
        kws.path = d
        with patch.object(wake_kws, 'FILES', ['en.phone', 'tokens.txt']):
            self.assertEqual(kws.pronounce(' hey mimikyu '), dict(
                text='HEY MIMI+KYU', tokens=['HH', 'EY1', 'M', 'IY1', 'M', 'IY0', 'K', 'Y', 'UW1']))
            for phrase, code in [('  ', 'wake_phrase_empty'), ('hey 1', 'wake_phrase_invalid'),
                                 ('hey qqq', 'wake_phrase_unpronounceable')]:
                with self.subTest(phrase=phrase), self.assertRaises(WakeError) as cm: kws.pronounce(phrase)
                self.assertEqual(cm.exception.code, code)
        (d / 'tokens.txt').unlink()
        with patch.object(wake_kws, 'FILES', ['en.phone', 'tokens.txt']), self.assertRaises(WakeError) as cm:
            kws.pronounce('hey mimikyu')
        self.assertEqual(cm.exception.code, 'wake_model_missing')


if __name__ == '__main__': unittest.main()
