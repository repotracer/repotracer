#!/usr/bin/env node
'use strict';

const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const os = require('os');

function platformKey() {
  const p = process.platform;
  const a = process.arch;
  if (p === 'darwin' && a === 'arm64') return 'darwin-arm64';
  if (p === 'darwin' && a === 'x64') return 'darwin-x64';
  if (p === 'linux' && a === 'x64') return 'linux-x64';
  if (p === 'linux' && a === 'arm64') return 'linux-arm64';
  if (p === 'win32' && a === 'x64') return 'windows-x64';
  return `${p}-${a}`;
}

function findBinary() {
  const ext = process.platform === 'win32' ? '.exe' : '';
  const name = `repotracer${ext}`;
  const candidates = [
    process.env.REPOTRACER_BIN,
    path.join(__dirname, '..', 'vendor', platformKey(), name),
    path.join(__dirname, '..', '..', '..', 'target', 'release', name),
    path.join(__dirname, '..', '..', '..', 'target', 'debug', name),
    name, // PATH
  ].filter(Boolean);

  for (const c of candidates) {
    if (c === name) return c;
    if (fs.existsSync(c)) return c;
  }
  return null;
}

const bin = findBinary();
if (!bin) {
  console.error(`repotracer: native binary not found for ${platformKey()}.`);
  console.error('Build from source: cargo install --path crates/cli');
  console.error('Or set REPOTRACER_BIN to the binary path.');
  process.exit(1);
}

const args = process.argv.slice(2);
const res = spawnSync(bin, args, { stdio: 'inherit' });
process.exit(res.status == null ? 1 : res.status);
