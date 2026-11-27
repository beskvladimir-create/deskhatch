// Minimal CLI test: forwards to a runtime binary given by DESKHATCH_BIN.
'use strict';
const { spawnSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

// The AppImage runtime carries FUSE inside, so the wrapper must not fall back to the slow
// self-extracting start just because the system has no libfuse.so.2. It mounts wherever
// /dev/fuse and a setuid-root fusermount are there, and only then may the fallback kick in.
{
  const { canMountFuse } = require('./index.js');
  const setuidRoot = { uid: 0, mode: 0o104755 };
  const world = (dev, dirs, stats) => ({
    existsSync: (p) => dev && p === '/dev/fuse',
    readdirSync: (d) => { if (!(d in dirs)) throw new Error('ENOENT'); return dirs[d]; },
    statSync: (p) => { if (!(p in stats)) throw new Error('ENOENT'); return stats[p]; },
  });
  const cases = [
    ['a desktop with fusermount3 and no libfuse2 mounts', true, true,
      { '/usr/bin': ['ls', 'fusermount3'] }, { '/usr/bin/fusermount3': setuidRoot }],
    ['an older desktop with fusermount mounts', true, true,
      { '/usr/bin': ['fusermount'] }, { '/usr/bin/fusermount': setuidRoot }],
    ['an unreadable PATH entry does not hide the helper behind it', true, true,
      { '/usr/bin': ['fusermount3'] }, { '/usr/bin/fusermount3': setuidRoot }, '/nope:/usr/bin'],
    ['no /dev/fuse: unpack instead', false, false,
      { '/usr/bin': ['fusermount3'] }, { '/usr/bin/fusermount3': setuidRoot }],
    ['no helper on the PATH: unpack instead', true, false, { '/usr/bin': ['ls'] }, {}],
    ['a helper that is not setuid root: unpack instead', true, false,
      { '/usr/bin': ['fusermount3'] }, { '/usr/bin/fusermount3': { uid: 0, mode: 0o100755 } }],
    ['a look-alike name is not a helper', true, false,
      { '/usr/bin': ['fusermount3-wrapper'] }, { '/usr/bin/fusermount3-wrapper': setuidRoot }],
  ];
  for (const [what, dev, want, dirs, stats, pathEnv = '/usr/bin'] of cases) {
    if (canMountFuse(world(dev, dirs, stats), pathEnv) !== want) throw new Error(`FUSE check: ${what}`);
  }
}

const bin = process.env.DESKHATCH_BIN;
if (!bin || !fs.existsSync(bin)) {
  console.log('skip: set DESKHATCH_BIN to a built runtime to run the CLI test (FUSE check ran)');
  process.exit(0);
}
const home = fs.mkdtempSync(path.join(os.tmpdir(), 'dh-'));
const env = { ...process.env, DESKHATCH_HOME: home, HOME: home };
const cli = path.join(__dirname, 'index.js');
const html = path.join(home, 'page.html');
fs.writeFileSync(html, '<!doctype html><title>CLI Test App</title><h1>x</h1>');

function run(args) {
  const r = spawnSync(process.execPath, [cli, ...args], { env, encoding: 'utf8' });
  if (r.status !== 0) { console.error(r.stdout, r.stderr); throw new Error(`exit ${r.status}: ${args.join(' ')}`); }
  return r.stdout;
}
run(['--version']);
const out = run(['install', html, '--no-launcher']);
if (!out.includes('cli-test-app')) throw new Error('install output unexpected: ' + out);
const list = run(['list']);
if (!list.includes('cli-test-app')) throw new Error('list missing app');
run(['export', 'cli-test-app', '--out', home]);
if (!fs.existsSync(path.join(home, 'cli-test-app.hatch'))) throw new Error('export missing');
run(['uninstall', 'cli-test-app']);

// A package folder may ship without the icon its manifest names: one must be drawn for it,
// otherwise the launcher ends up with the system placeholder.
const folder = path.join(home, 'noicon');
fs.mkdirSync(folder);
fs.writeFileSync(
  path.join(folder, 'app.json'),
  JSON.stringify({ schema: 1, id: 'noicon', name: 'No Icon', version: '1.0.0', permissions: 'web' })
);
fs.writeFileSync(path.join(folder, 'index.html'), '<!doctype html><title>No Icon</title><h1>x</h1>');
run(['install', folder, '--no-launcher']);
if (!fs.existsSync(path.join(home, 'apps', 'noicon', 'icon.png'))) throw new Error('default icon missing');
run(['relink']);
run(['uninstall', 'noicon']);

// The runtime is verified against the checksums published with the signed release, so the npm
// package must carry them for its own version, and a custom runtime URL must declare its sum.
const sums = require('./checksums.json');
const pkgVersion = require('../package.json').version;
if (sums.version !== pkgVersion) throw new Error(`checksums.json is for ${sums.version}, package is ${pkgVersion}`);
for (const f of [
  `deskhatch-v${pkgVersion}-darwin-arm64.tar.gz`,
  `deskhatch-v${pkgVersion}-darwin-x64.tar.gz`,
  `deskhatch-v${pkgVersion}-linux-arm64.AppImage`,
  `deskhatch-v${pkgVersion}-linux-x64.AppImage`,
]) {
  if (!/^[0-9a-f]{64}$/.test(sums.files[f] || '')) throw new Error(`checksums.json has no sha256 for ${f}`);
}
const noSum = spawnSync(process.execPath, [cli, 'list'], {
  encoding: 'utf8',
  env: { ...env, DESKHATCH_BIN: '', DESKHATCH_HOME: home, DESKHATCH_RUNTIME_URL: 'https://example.invalid/runtime.tar.gz' },
});
if (noSum.status === 0) throw new Error('a custom runtime URL was accepted without a checksum');
if (!/needs DESKHATCH_RUNTIME_SHA256/.test(noSum.stderr)) throw new Error(`unexpected refusal: ${noSum.stderr}`);
if (fs.existsSync(path.join(home, 'runtime'))) throw new Error('a refused runtime left files behind');

console.log('cli test ok');
