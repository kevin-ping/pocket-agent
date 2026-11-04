// Voice setup state ordering (snapshot vs events). Run: node --test scripts/test_voice_setup.mjs
// The repo has no frontend test runner, so transpile the module with the
// project's TypeScript into a temp dir and import it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import ts from 'typescript';

const dir = mkdtempSync(join(tmpdir(), 'voice-setup-'));
const src = readFileSync(new URL('../src/lib/voiceSetup.ts', import.meta.url), 'utf8');
const out = ts.transpileModule(src, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
writeFileSync(join(dir, 'voiceSetup.mjs'), out);
const { createSetupSequencer } = await import(pathToFileURL(join(dir, 'voiceSetup.mjs')).href);

const state = (seq, s, phase = '', detail = '') => ({ seq, state: s, phase, detail });

test('a snapshot older than an applied event does not overwrite it', () => {
  const applied = [];
  const apply = createSetupSequencer((s) => applied.push(s));
  apply(state(3, 'error', 'finding-python', 'no python'));   // event arrived first
  assert.equal(apply(state(2, 'installing')), false);        // snapshot read before it
  assert.deepEqual(applied.map((s) => s.state), ['error']);
});

test('a snapshot newer than the events seen so far is applied', () => {
  const applied = [];
  const apply = createSetupSequencer((s) => applied.push(s));
  assert.equal(apply(state(5, 'error', 'installing-deps', 'pip failed')), true);
  assert.equal(apply(state(5, 'error', 'installing-deps', 'pip failed')), false, 'the same seq twice is applied once');
  assert.equal(apply(state(6, 'ready')), true);
  assert.deepEqual(applied.map((s) => s.seq), [5, 6]);
});

test('the idle snapshot (seq 0) of a backend that reported nothing is ignored', () => {
  const applied = [];
  const apply = createSetupSequencer((s) => applied.push(s));
  assert.equal(apply(state(0, 'idle')), false);
  assert.equal(applied.length, 0);
});
