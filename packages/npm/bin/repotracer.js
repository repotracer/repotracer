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
  // Vendored binary first. The target/ paths only exist in a source checkout and
  // let the benchmark harness run this launcher without publishing. A bare name on
  // PATH is deliberately NOT a candidate: silently running an unrelated or stale
  // `repotracer` is worse than failing with instructions.
  const candidates = [
    process.env.REPOTRACER_BIN,
    path.join(__dirname, '..', 'vendor', platformKey(), name),
    path.join(__dirname, '..', '..', '..', 'target', 'release', name),
    path.join(__dirname, '..', '..', '..', 'target', 'debug', name),
  ].filter(Boolean);

  return candidates.find(candidate => fs.existsSync(candidate)) || null;
}

function persistForSetup(bin, args, home = os.homedir()) {
  if (!path.isAbsolute(bin) || !args.includes('setup') || args.includes('--dry-run')) return bin;

  const name = process.platform === 'win32' ? 'repotracer.exe' : 'repotracer';
  const directory = path.join(home, '.repotracer', 'bin');
  const destination = path.join(directory, name);
  if (path.resolve(bin) === path.resolve(destination)) return bin;

  const temporary = `${destination}.tmp-${process.pid}`;
  fs.mkdirSync(directory, { recursive: true });
  try {
    fs.copyFileSync(bin, temporary);
    if (process.platform !== 'win32') fs.chmodSync(temporary, 0o755);
    try {
      fs.renameSync(temporary, destination);
    } catch (error) {
      if (!['EEXIST', 'EPERM'].includes(error.code)) throw error;
      const displaced = `${destination}.old-${process.pid}`;
      fs.renameSync(destination, displaced);
      try {
        fs.renameSync(temporary, destination);
      } catch (installError) {
        fs.renameSync(displaced, destination);
        throw installError;
      }
      try { fs.rmSync(displaced, { force: true }); } catch {}
    }
  } finally {
    fs.rmSync(temporary, { force: true });
  }
  // The setup TUI reports the final binary path itself; stay quiet unless debugging.
  if (process.env.REPOTRACER_DEBUG) console.log(`repotracer: installed CLI at ${destination}`);
  return destination;
}

function main(args = process.argv.slice(2)) {
  const found = findBinary();
  if (!found) {
    console.error(`repotracer: native binary not found for ${platformKey()}.`);
    console.error('Reinstall with: npm install -g repotracer');
    return 1;
  }

  const bin = persistForSetup(found, args);
  const result = spawnSync(bin, args, { stdio: 'inherit' });
  if (result.error) console.error(`repotracer: could not start: ${result.error.message}`);
  return result.status == null ? 1 : result.status;
}

if (require.main === module) process.exit(main());

module.exports = { findBinary, main, persistForSetup, platformKey };
