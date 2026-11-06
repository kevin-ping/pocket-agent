// Dev port check (clear-dev-port.js). Run: node --test scripts/test_dev_port.mjs
// Real listeners on a random port with the real lsof, plus injected tool failures.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { checkPort } from './clear-dev-port.js';

const mac = process.platform === 'darwin';

/** A listener in a child process (so the PID is not ours) bound to `host`. */
async function listenIn(host) {
  const child = spawn(process.execPath, ['-e', `
    const s = require('net').createServer().listen(0, ${JSON.stringify(host)}, () => console.log(s.address().port));
  `], { stdio: ['ignore', 'pipe', 'inherit'] });
  const port = await new Promise((resolve) => child.stdout.once('data', (d) => resolve(Number(d))));
  return { child, port };
}

async function freePort() {
  const s = createServer().listen(0, '127.0.0.1');
  await new Promise((r) => s.once('listening', r));
  const { port } = s.address();
  await new Promise((r) => s.close(r));
  return port;
}

test('a free port passes', { skip: !mac }, async () => {
  assert.deepEqual(checkPort(await freePort()), { state: 'free' });
});

for (const host of ['127.0.0.1', '::1']) {
  test(`an ${host === '::1' ? 'IPv6' : 'IPv4'}-only listener is found with its PID and command`, { skip: !mac }, async () => {
    const { child, port } = await listenIn(host);
    try {
      const r = checkPort(port);
      assert.equal(r.state, 'busy');
      assert.deepEqual(r.listeners.map((l) => l.pid), [String(child.pid)]);
      assert.match(r.listeners[0].command, /node/);
    } finally {
      child.kill();
    }
  });
}

const ok = (stdout, status = 0) => ({ status, stdout, stderr: '', error: undefined });

test('lsof missing is unknown, not free', () => {
  const enoent = Object.assign(new Error('spawnSync lsof ENOENT'), { code: 'ENOENT' });
  const r = checkPort(1420, 'darwin', () => ({ status: null, stdout: '', stderr: '', error: enoent }));
  assert.equal(r.state, 'unknown');
  assert.match(r.reason, /ENOENT/);
});

test('lsof failing for another reason is unknown', () => {
  const r = checkPort(1420, 'darwin', () => ({ status: 1, stdout: '', stderr: 'lsof: unsupported option', error: undefined }));
  assert.equal(r.state, 'unknown');
  assert.match(r.reason, /unsupported option/);
  assert.equal(checkPort(1420, 'darwin', () => ok('', 2)).state, 'unknown');
  assert.equal(checkPort(1420, 'darwin', () => ok('garbage\n', 0)).state, 'unknown');
});

test('a PID still counts when ps cannot name it, or lsof also warned', () => {
  const r = checkPort(1420, 'darwin', (tool) => (tool === 'lsof' ? ok('4242\n') : { status: 1, stdout: '', stderr: '', error: undefined }));
  assert.deepEqual(r, { state: 'busy', listeners: [{ pid: '4242', command: '' }] });
  const warned = checkPort(1420, 'darwin', (tool) => (tool === 'lsof' ? { status: 1, stdout: '4242\n', stderr: 'lsof: WARNING: can\'t stat()', error: undefined } : ok('python3 x.py\n')));
  assert.deepEqual(warned, { state: 'busy', listeners: [{ pid: '4242', command: 'python3 x.py' }] });
});

test('nothing matching (lsof exit 1, silent) is free', () => {
  assert.deepEqual(checkPort(1420, 'darwin', () => ok('', 1)), { state: 'free' });
});

test('windows: netstat failure is unknown; :14200 is not :1420', () => {
  assert.equal(checkPort(1420, 'win32', () => ({ status: null, stdout: '', stderr: '', error: new Error('ENOENT') })).state, 'unknown');
  const out = '  TCP    0.0.0.0:14200   0.0.0.0:0   LISTENING   11\n  TCP    0.0.0.0:1420    0.0.0.0:0   LISTENING   22\n';
  assert.deepEqual(checkPort(1420, 'win32', () => ok(out)), { state: 'busy', listeners: [{ pid: '22', command: '' }] });
});

test('windows: listeners are found whatever language the state column is in', () => {
  const out = [
    '  TCP    127.0.0.1:1420     0.0.0.0:0          侦听            33',
    '  TCP    [::1]:1420         [::]:0             ABHÖREN         44',
    '  TCP    127.0.0.1:1420     127.0.0.1:50000    ESTABLISHED     33',
    '  TCP    127.0.0.1:50000    127.0.0.1:1420     ESTABLISHED     55',
    '  UDP    0.0.0.0:1420       *:*                                66',
  ].join('\r\n');
  assert.deepEqual(checkPort(1420, 'win32', () => ok(out)), {
    state: 'busy', listeners: [{ pid: '33', command: '' }, { pid: '44', command: '' }],
  });
  assert.equal(checkPort(50000, 'win32', () => ok(out)).state, 'free');
});
