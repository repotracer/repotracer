'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const { install, platformKey } = require('../scripts/fetch-binary');
const { persistForSetup } = require('../bin/repotracer');

function response(body) {
  const bytes = Buffer.from(body);
  return {
    ok: true,
    status: 200,
    arrayBuffer: async () => Uint8Array.from(bytes).buffer,
  };
}

test('installer downloads and verifies the platform binary', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'repotracer-npm-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const binary = Buffer.from('native-binary');
  const hash = crypto.createHash('sha256').update(binary).digest('hex');
  const fetchImpl = async (url) => response(
    url.endsWith('/SHA256SUMS')
      ? `${hash}  ./repotracer-darwin-arm64/repotracer-darwin-arm64\n`
      : binary,
  );

  const destination = await install({
    root,
    platform: 'darwin',
    arch: 'arm64',
    baseUrl: 'https://releases.example/v0.1.0',
    fetchImpl,
    quiet: true,
  });

  assert.equal(fs.readFileSync(destination, 'utf8'), 'native-binary');
});

test('installer rejects a checksum mismatch', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'repotracer-checksum-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const fetchImpl = async (url) => response(
    url.endsWith('/SHA256SUMS')
      ? `${'0'.repeat(64)}  repotracer-linux-x64\n`
      : 'tampered-binary',
  );

  await assert.rejects(
    install({ root, platform: 'linux', arch: 'x64', fetchImpl, quiet: true }),
    /checksum mismatch/,
  );
});

test('installer honours the release base URL override', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'repotracer-mirror-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const previous = process.env.REPOTRACER_RELEASE_BASE_URL;
  process.env.REPOTRACER_RELEASE_BASE_URL = 'https://mirror.example/repotracer';
  t.after(() => {
    if (previous === undefined) delete process.env.REPOTRACER_RELEASE_BASE_URL;
    else process.env.REPOTRACER_RELEASE_BASE_URL = previous;
  });

  const binary = Buffer.from('mirrored-binary');
  const hash = crypto.createHash('sha256').update(binary).digest('hex');
  const seen = [];
  const fetchImpl = async (url) => {
    seen.push(url);
    return response(url.endsWith('/SHA256SUMS') ? `${hash}  repotracer-linux-x64\n` : binary);
  };

  await install({ root, platform: 'linux', arch: 'x64', fetchImpl, quiet: true });
  assert.ok(seen.length > 0);
  assert.ok(seen.every(url => url.startsWith('https://mirror.example/repotracer/')));
});

test('installer rejects an unsupported platform', async () => {
  assert.throws(() => platformKey('freebsd', 'x64'), /unsupported platform/);
});

test('setup persists the npx binary but dry-run does not', (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'repotracer-setup-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const source = path.join(root, process.platform === 'win32' ? 'source.exe' : 'source');
  const home = path.join(root, 'home');
  const installed = path.join(
    home,
    '.repotracer',
    'bin',
    process.platform === 'win32' ? 'repotracer.exe' : 'repotracer',
  );
  fs.writeFileSync(source, 'native-binary', { mode: 0o755 });

  assert.equal(persistForSetup(source, ['setup', '--dry-run'], home), source);
  assert.equal(fs.existsSync(installed), false);

  const destination = persistForSetup(source, ['setup'], home);
  assert.equal(fs.readFileSync(destination, 'utf8'), 'native-binary');

  fs.writeFileSync(source, 'updated-binary', { mode: 0o755 });
  assert.equal(persistForSetup(source, ['setup'], home), destination);
  assert.equal(fs.readFileSync(destination, 'utf8'), 'updated-binary');
});
