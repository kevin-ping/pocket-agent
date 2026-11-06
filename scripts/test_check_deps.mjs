// npm dependency check (check-deps.js). Run: node --test scripts/test_check_deps.mjs
// Each case builds a small project in a temp directory.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { checkDeps } from './check-deps.js';

/** Project with deps a@1.0.0 (dependency) and b@2.0.0 (devDependency); `installed` maps name → version. */
function project({ installed = { a: '1.0.0', b: '2.0.0' }, lock = true, modules = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'check-deps-'));
  writeFileSync(join(root, 'package.json'), JSON.stringify({ dependencies: { a: '^1' }, devDependencies: { b: '^2' } }));
  if (lock) writeFileSync(join(root, 'package-lock.json'), JSON.stringify({
    packages: { '': {}, 'node_modules/a': { version: '1.0.0' }, 'node_modules/b': { version: '2.0.0' } },
  }));
  if (modules) {
    mkdirSync(join(root, 'node_modules'));
    for (const [name, version] of Object.entries(installed)) {
      mkdirSync(join(root, 'node_modules', name));
      writeFileSync(join(root, 'node_modules', name, 'package.json'), JSON.stringify({ name, version }));
    }
  }
  return root;
}

function check(opts) {
  const root = project(opts);
  try { return checkDeps(root); } finally { rmSync(root, { recursive: true, force: true }); }
}

test('installed versions match the lock', () => {
  assert.deepEqual(check(), { state: 'ok' });
});

test('a missing package is reported', () => {
  assert.deepEqual(check({ installed: { a: '1.0.0' } }), {
    state: 'stale', problems: [{ name: 'b', locked: '2.0.0', installed: null }],
  });
});

test('a different installed version is reported', () => {
  assert.deepEqual(check({ installed: { a: '0.9.0', b: '2.0.0' } }), {
    state: 'stale', problems: [{ name: 'a', locked: '1.0.0', installed: '0.9.0' }],
  });
});

test('no node_modules is stale', () => {
  assert.equal(check({ modules: false }).state, 'stale');
  assert.equal(check({ modules: false }).noModules, true);
});

test('no package-lock.json is unknown, not ok', () => {
  assert.equal(check({ lock: false }).state, 'unknown');
});
