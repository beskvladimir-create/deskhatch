#!/usr/bin/env node
// deskhatch CLI: a thin npx-friendly front for the native runtime.
// It downloads the signed runtime for this OS once (into ~/.deskhatch/runtime) and forwards commands.
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const https = require('https');
const crypto = require('crypto');
const { spawn, spawnSync } = require('child_process');

const pkg = require('../package.json');
const VERSION = process.env.DESKHATCH_RUNTIME_VERSION || pkg.version;
const REPO = 'beskvladimir-create/deskhatch';
const HOME = process.env.DESKHATCH_HOME || path.join(os.homedir(), '.deskhatch');
const RUNTIME_DIR = path.join(HOME, 'runtime', VERSION);

function assetName() {
  const arch = process.arch === 'arm64' ? 'arm64' : process.arch === 'x64' ? 'x64' : null;
  if (!arch) return null;
  if (process.platform === 'darwin') return `deskhatch-v${VERSION}-darwin-${arch}.tar.gz`;
  if (process.platform === 'linux') return `deskhatch-v${VERSION}-linux-${arch}.AppImage`;
  return null;
}

function binaryPath() {
  if (process.env.DESKHATCH_BIN) return process.env.DESKHATCH_BIN;
  if (process.platform === 'darwin') return path.join(RUNTIME_DIR, 'Deskhatch.app', 'Contents', 'MacOS', 'deskhatch');
  if (process.platform === 'linux') return path.join(RUNTIME_DIR, assetName() || 'deskhatch.AppImage');
  return null;
}

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 8) return reject(new Error('too many redirects'));
    https.get(url, { headers: { 'user-agent': `deskhatch-cli/${VERSION}` } }, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode) && res.headers.location) {
        res.resume();
        return resolve(download(res.headers.location, dest, redirects + 1));
      }
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
      }
      const total = Number(res.headers['content-length'] || 0);
      let got = 0, lastPct = -1;
      const out = fs.createWriteStream(dest);
      res.on('data', (c) => {
        got += c.length;
        if (total && process.stderr.isTTY) {
          const pct = Math.floor((got / total) * 100);
          if (pct !== lastPct && pct % 5 === 0) { lastPct = pct; process.stderr.write(`\r  downloading runtime… ${pct}%`); }
        }
      });
      res.pipe(out);
      out.on('finish', () => { if (process.stderr.isTTY) process.stderr.write('\r'); out.close(resolve); });
      out.on('error', reject);
    }).on('error', reject);
  });
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

/// The checksum the downloaded runtime must have. Preferred: the list shipped inside this npm
/// package (written at publish time from the signed release). Otherwise the release's own
/// SHA256SUMS.txt. A custom DESKHATCH_RUNTIME_URL must come with DESKHATCH_RUNTIME_SHA256.
async function expectedSum(asset) {
  if (process.env.DESKHATCH_RUNTIME_URL) {
    const s = (process.env.DESKHATCH_RUNTIME_SHA256 || '').trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(s)) fail('DESKHATCH_RUNTIME_URL needs DESKHATCH_RUNTIME_SHA256 (the sha256 of that file)');
    return s;
  }
  try {
    const shipped = require('./checksums.json');
    if (shipped && shipped.version === VERSION && shipped.files && shipped.files[asset]) return shipped.files[asset];
  } catch (_) { /* not shipped: fall back to the release's list */ }
  const tmp = path.join(RUNTIME_DIR, 'SHA256SUMS.txt.part');
  await download(`https://github.com/${REPO}/releases/download/v${VERSION}/SHA256SUMS.txt`, tmp);
  const line = fs.readFileSync(tmp, 'utf8').split('\n').map((l) => l.trim().split(/\s+/)).find((p) => p[1] === asset);
  fs.rmSync(tmp, { force: true });
  if (!line || !/^[0-9a-f]{64}$/.test(line[0])) fail(`the release has no checksum for ${asset}; not installing an unverified runtime`);
  return line[0];
}

async function ensureRuntime({ force = false } = {}) {
  const bin = binaryPath();
  if (!bin) {
    fail(`deskhatch runtime is not available for ${process.platform}/${process.arch} yet.\n` +
      `Windows is planned — subscribe to https://github.com/${REPO}/issues/1`);
  }
  if (!force && fs.existsSync(bin)) return bin;
  const asset = assetName();
  const url = process.env.DESKHATCH_RUNTIME_URL || `https://github.com/${REPO}/releases/download/v${VERSION}/${asset}`;
  // A custom URL must declare its checksum before anything is fetched, so a refusal costs
  // neither a download nor a leftover .part file.
  if (process.env.DESKHATCH_RUNTIME_URL &&
      !/^[0-9a-f]{64}$/.test((process.env.DESKHATCH_RUNTIME_SHA256 || '').trim().toLowerCase())) {
    fail('DESKHATCH_RUNTIME_URL needs DESKHATCH_RUNTIME_SHA256 (the sha256 of that file)');
  }
  fs.mkdirSync(RUNTIME_DIR, { recursive: true });
  const tmp = path.join(RUNTIME_DIR, asset + '.part');
  process.stderr.write(`  fetching deskhatch runtime v${VERSION} for ${process.platform}-${process.arch}\n  ${url}\n`);
  try {
    await download(url, tmp);
  } catch (e) {
    fail(`could not download the runtime: ${e.message}\n` +
      `If this version has no release yet, build it yourself: see https://github.com/${REPO}#build-from-source`);
  }
  let want;
  try {
    want = await expectedSum(asset);
  } catch (e) {
    fs.rmSync(tmp, { force: true });
    fail(`could not get the runtime checksum: ${e.message}`);
  }
  const got = sha256(tmp);
  if (got !== want) {
    fs.rmSync(tmp, { force: true });
    fail(`the downloaded runtime does not match its checksum (expected ${want}, got ${got}); not installing it`);
  }
  if (process.platform === 'darwin') {
    fs.rmSync(path.join(RUNTIME_DIR, 'Deskhatch.app'), { recursive: true, force: true });
    const r = spawnSync('tar', ['-xzf', tmp, '-C', RUNTIME_DIR], { stdio: 'inherit' });
    if (r.status !== 0) fail('could not unpack the runtime archive');
    fs.rmSync(tmp, { force: true });
  } else {
    fs.renameSync(tmp, bin);
    fs.chmodSync(bin, 0o755);
  }
  if (!fs.existsSync(bin)) fail(`runtime unpacked but ${bin} is missing`);
  process.stderr.write(`  runtime ready: ${bin}\n`);
  return bin;
}

function fail(msg) {
  process.stderr.write(`deskhatch: ${msg}\n`);
  process.exit(1);
}

function usage() {
  process.stdout.write(`deskhatch ${VERSION} — turn HTML into real desktop apps

  npx deskhatch install <page.html | app-folder | file.hatch> [--name N] [--standalone] [--run]
  npx deskhatch new "<what the app should do>" [--name N] [--tier web|desktop|full] [--model M] [--run]
  npx deskhatch run <id>            start an installed app
  npx deskhatch dev <folder>        run an app folder with live reload
  npx deskhatch gallery             open the gallery of installed apps
  npx deskhatch list | uninstall <id> | export <id> [--out dir]
  npx deskhatch config [--provider ollama|openai|...] [--model M] [--endpoint URL]
  npx deskhatch key <provider>      store an API key in the OS keychain (read from stdin)
  npx deskhatch models | doctor
  npx deskhatch update              re-download the runtime and re-create the launchers
`);
}

async function main() {
  const args = process.argv.slice(2);
  const cmd = args[0];
  if (!cmd || cmd === '-h' || cmd === '--help' || cmd === 'help') return usage();
  if (cmd === '--version' || cmd === '-V') return process.stdout.write(VERSION + '\n');

  if (cmd === 'update') {
    const bin = await ensureRuntime({ force: true });
    // Standalone apps hold their own copy of the runtime, so rebuild every launcher.
    const r = spawnSync(bin, ['relink'], { stdio: 'inherit' });
    process.exit(r.status ?? 0);
  }
  const bin = await ensureRuntime();

  let fwd;
  let detach = false;
  switch (cmd) {
    case 'gallery': fwd = []; detach = true; break;
    case 'run': if (!args[1]) return usage(); fwd = ['--app', args[1]]; detach = true; break;
    case 'dev': if (!args[1]) return usage(); fwd = ['--dev', path.resolve(args[1])]; break;
    case 'install': if (!args[1]) return usage(); fwd = ['install', path.resolve(args[1]), ...args.slice(2)]; break;
    default: fwd = args;
  }

  const env = { ...process.env };
  if (process.platform === 'linux' && !fs.existsSync('/dev/fuse') && !env.APPIMAGE_EXTRACT_AND_RUN) {
    env.APPIMAGE_EXTRACT_AND_RUN = '1'; // no FUSE here (containers, some distros): let the AppImage self-extract
  }
  if (detach) {
    const child = spawn(bin, fwd, { detached: true, stdio: 'ignore', env });
    child.unref();
    return;
  }
  const child = spawn(bin, fwd, { stdio: 'inherit', env });
  child.on('exit', (code) => process.exit(code ?? 0));
}

main().catch((e) => fail(e.message));
