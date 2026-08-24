#!/usr/bin/env node
'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const pkg = require('../package.json');

function platformKey(platform = process.platform, arch = process.arch) {
  const supported = {
    'darwin-arm64': 'darwin-arm64',
    'darwin-x64': 'darwin-x64',
    'linux-arm64': 'linux-arm64',
    'linux-x64': 'linux-x64',
    'win32-x64': 'windows-x64',
  };
  const key = supported[`${platform}-${arch}`];
  if (!key) throw new Error(`unsupported platform: ${platform}-${arch}`);
  return key;
}

async function download(fetchImpl, url) {
  const response = await fetchImpl(url, {
    headers: { 'User-Agent': `repotracer-npm/${pkg.version}` },
  });
  if (!response.ok) throw new Error(`${url} returned HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

function expectedChecksum(sums, asset) {
  for (const line of sums.split(/\r?\n/)) {
    const match = line.trim().match(/^([a-fA-F0-9]{64})\s+(.+)$/);
    if (match && path.basename(match[2]) === asset) return match[1].toLowerCase();
  }
  throw new Error(`${asset} is missing from SHA256SUMS`);
}

async function install(options = {}) {
  const platform = options.platform || process.platform;
  const arch = options.arch || process.arch;
  const key = platformKey(platform, arch);
  const executable = platform === 'win32' ? 'repotracer.exe' : 'repotracer';
  const asset = `repotracer-${key}${executable.endsWith('.exe') ? '.exe' : ''}`;
  const root = options.root || path.join(__dirname, '..');
  const baseUrl = options.baseUrl
    || process.env.REPOTRACER_RELEASE_BASE_URL
    || `https://github.com/repotracer/repotracer/releases/download/v${pkg.version}`;
  const fetchImpl = options.fetchImpl || globalThis.fetch;

  if (!fetchImpl) throw new Error('Node.js 18 or newer is required');
  if (!options.quiet) console.log(`repotracer: downloading v${pkg.version} for ${key}...`);

  const [binary, sums] = await Promise.all([
    download(fetchImpl, `${baseUrl}/${asset}`),
    download(fetchImpl, `${baseUrl}/SHA256SUMS`),
  ]);
  const expected = expectedChecksum(sums.toString('utf8'), asset);
  const actual = crypto.createHash('sha256').update(binary).digest('hex');
  if (actual !== expected) throw new Error(`checksum mismatch for ${asset}`);

  const directory = path.join(root, 'vendor', key);
  const destination = path.join(directory, executable);
  const temporary = `${destination}.tmp-${process.pid}`;
  fs.mkdirSync(directory, { recursive: true });
  try {
    fs.writeFileSync(temporary, binary, { flag: 'wx', mode: 0o755 });
    try {
      fs.renameSync(temporary, destination);
    } catch (error) {
      if (!['EEXIST', 'EPERM'].includes(error.code)) throw error;
      fs.rmSync(destination, { force: true });
      fs.renameSync(temporary, destination);
    }
    if (process.platform !== 'win32') fs.chmodSync(destination, 0o755);
  } finally {
    fs.rmSync(temporary, { force: true });
  }

  if (!options.quiet) console.log(`repotracer: installed ${destination}`);
  return destination;
}

if (require.main === module) {
  install().catch((error) => {
    console.error(`repotracer: installation failed: ${error.message}`);
    console.error(`repotracer: GitHub Release v${pkg.version} must exist before npm installation.`);
    process.exit(1);
  });
}

module.exports = { expectedChecksum, install, platformKey };
