// Idle dialog hint (wake word / double-click). Run: node --test scripts/test_idle_hint.mjs
// The repo has no frontend test runner, so transpile i18n.ts with the
// project's TypeScript into a temp dir and import it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import ts from 'typescript';

const dir = mkdtempSync(join(tmpdir(), 'idle-hint-'));
const src = readFileSync(new URL('../src/lib/i18n.ts', import.meta.url), 'utf8');
writeFileSync(join(dir, 'i18n.mjs'), ts.transpileModule(src, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText);
const { idleHint } = await import(pathToFileURL(join(dir, 'i18n.mjs')).href);
const LANGS = ['zh', 'en', 'ja', 'ko', 'fr', 'de', 'es'];
const base = { ui_lang: 'zh', hotkey_name: 'fn', double_click_to_record: false, wake_word_enabled: false, wake_phrase: '' };

test('zh hints for each wake / double-click combination', () => {
  assert.equal(idleHint(base), '按下 fn 说话，再按一次结束');
  assert.equal(idleHint({ ...base, double_click_to_record: true }), '双击 fn 说话，再双击结束');
  assert.equal(idleHint({ ...base, wake_word_enabled: true, wake_phrase: '桃子桃子' }), '说出唤醒词唤醒我，或按下 fn 说话，再按一次结束');
  assert.equal(idleHint({ ...base, wake_word_enabled: true, wake_phrase: '桃子桃子', double_click_to_record: true }), '说出唤醒词唤醒我，或双击 fn 说话，再双击结束');
});

test('wake enabled with an empty or blank phrase falls back to the plain hint', () => {
  for (const wake_phrase of ['', '   ']) {
    assert.equal(idleHint({ ...base, wake_word_enabled: true, wake_phrase }), idleHint(base));
  }
  assert.equal(idleHint({ ...base, wake_phrase: '桃子桃子' }), idleHint(base));
});

test('every language fills all placeholders and never reveals the wake phrase', () => {
  for (const ui_lang of LANGS) {
    for (const wake_word_enabled of [false, true]) {
      for (const double_click_to_record of [false, true]) {
        const s = { ...base, ui_lang, wake_word_enabled, double_click_to_record, wake_phrase: 'star start' };
        const hint = idleHint(s);
        assert.doesNotMatch(hint, /\{|\}|undefined/, `${ui_lang}: ${hint}`);
        assert.match(hint, /fn/, `${ui_lang}: ${hint}`);
        assert.doesNotMatch(hint, /star start/, `${ui_lang}: ${hint}`);
        // The chat input row was removed; hints must not point at it.
        assert.doesNotMatch(hint, /输入|type below|入力|입력|tape|tipp|escrib/i, `${ui_lang}: ${hint}`);
        assert.equal(hint !== idleHint({ ...s, wake_word_enabled: false }), wake_word_enabled, `${ui_lang}: ${hint}`);
      }
    }
  }
  assert.equal(idleHint({ ...base, ui_lang: 'en', wake_word_enabled: true, wake_phrase: 'star start', double_click_to_record: true }),
    'Say the wake word to wake me. Double-click fn to talk, double-click again to stop');
});
