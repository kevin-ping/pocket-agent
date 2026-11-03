// Wake error code → localized message mapping. Run: node --test scripts/test_wake_errors.mjs
// The repo has no frontend test runner, so transpile the modules with the
// project's TypeScript into a temp dir and import them.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import ts from 'typescript';

const dir = mkdtempSync(join(tmpdir(), 'wake-errors-'));
for (const name of ['wakeErrors', 'i18n', 'settingsI18n']) {
  const src = readFileSync(new URL(`../src/lib/${name}.ts`, import.meta.url), 'utf8');
  const out = ts.transpileModule(src, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
  writeFileSync(join(dir, `${name}.mjs`), out.replace(/from '\.\/(\w+)'/g, "from './$1.mjs'"));
}
const { WAKE_ERROR_CODES, parseWakeError, wakeErrorMessage, wakeSaveNotice } = await import(pathToFileURL(join(dir, 'wakeErrors.mjs')).href);
const { convLabelsLang } = await import(pathToFileURL(join(dir, 'i18n.mjs')).href);
const LANGS = ['zh', 'en', 'ja', 'ko', 'fr', 'de', 'es'];
const err = (code, params = {}) => JSON.stringify({ wake_error: code, params });
console.warn = () => {};

test('every code has a message in every language without raw details', () => {
  for (const lang of LANGS) {
    for (const code of WAKE_ERROR_CODES) {
      const msg = wakeErrorMessage(lang, err(code, { word: 'zorblax', current: '12.7.6', required: '13.4' }));
      assert.ok(msg.length > 10, `${lang}/${code}`);
      assert.doesNotMatch(msg, /undefined|wake_|\{|\bHTTP\b|\b[45]\d\d\b|dlopen/, `${lang}/${code}: ${msg}`);
    }
  }
});

test('params are shown to the user', () => {
  assert.match(wakeErrorMessage('zh', err('wake_phrase_unpronounceable', { word: 'zorblax' })), /zorblax/);
  const os = wakeErrorMessage('en', err('wake_engine_unsupported_os', { current: '12.7.6', required: '13.4' }));
  assert.match(os, /12\.7\.6/);
  assert.match(os, /13\.4/);
});

test('empty phrase reuses the main-window hint', () => {
  for (const lang of LANGS) {
    assert.equal(wakeErrorMessage(lang, err('wake_phrase_empty')), convLabelsLang(lang).wakeNeedsPhrase);
  }
});

test('uncoded and unknown errors fall back to a generic message', () => {
  const generic = wakeErrorMessage('en', err('wake_service_error'));
  for (const raw of ['Wake service 503 Service Unavailable: {"detail":"dlopen(...)"}', err('something_new'), '']) {
    assert.equal(wakeErrorMessage('en', raw), generic);
  }
  assert.equal(parseWakeError('{"detail":"x"}'), null);
  assert.equal(parseWakeError(err('something_new')).code, 'wake_service_error');
});

test('save notice says the settings were saved', () => {
  assert.match(wakeSaveNotice('zh', err('wake_engine_unavailable')), /^设置已保存，但唤醒未启动：/);
  assert.match(wakeSaveNotice('en', err('wake_phrase_empty')), /^Settings saved\. /);
});

test('unknown UI language falls back to English', () => {
  assert.equal(wakeErrorMessage('xx', err('wake_model_missing')), wakeErrorMessage('en', err('wake_model_missing')));
});
