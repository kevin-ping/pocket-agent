// macOS 12.0 ships Safari 15.0's WebKit; the Vite target (safari13) lowers syntax
// but adds no runtime APIs. App code must not call APIs that arrived later.
// Run: node --test scripts/test_webkit_compat.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));

// Safari 15.4+: Array/String .at, Object.hasOwn, structuredClone, findLast(Index),
// crypto.randomUUID. Safari 16+: Array toSorted/toReversed/toSpliced/with.
const NEWER_APIS = /\.at\(|Object\.hasOwn|structuredClone|\.findLast(Index)?\(|randomUUID|\.toSorted\(|\.toReversed\(|\.toSpliced\(/;

// MeshFace only feature-tests these to fall back from the mesh avatar.
const ALLOWED = new Set(['src/lib/components/MeshFace.svelte']);

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return files(path);
    return /\.(ts|js|svelte)$/.test(name) ? [path] : [];
  });
}

test('app code calls no API newer than Safari 15.0', () => {
  const hits = [];
  for (const path of files(join(root, 'src'))) {
    const rel = relative(root, path);
    if (ALLOWED.has(rel)) continue;
    readFileSync(path, 'utf8').split('\n').forEach((line, i) => {
      if (NEWER_APIS.test(line)) hits.push(`${rel}:${i + 1}: ${line.trim()}`);
    });
  }
  assert.deepEqual(hits, []);
});

test('the guard pattern catches the calls it is meant to', () => {
  for (const line of ['steps.at(-1)', 'Object.hasOwn(a, b)', 'structuredClone(x)', 'a.findLast(f)', 'a.findLastIndex(f)', 'crypto.randomUUID()']) {
    assert.match(line, NEWER_APIS, line);
  }
  assert.doesNotMatch('steps[steps.length - 1]', NEWER_APIS);
});
