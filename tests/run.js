// Regression tests for the bugs deskhatch has actually had. Every case here failed once.
//
//   DESKHATCH_BIN=runtime/target/debug/deskhatch node tests/run.js
//
// DESKHATCH_BIN    the runtime binary (required)
// DESKHATCH_BUNDLE a built Deskhatch.app; on macOS one is improvised from the binary when unset
// DESKHATCH_GUI=1  also run the cases that open a real window
'use strict';
const { spawn, spawnSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const BIN = process.env.DESKHATCH_BIN;
const BUNDLE = process.env.DESKHATCH_BUNDLE;
const GUI = process.env.DESKHATCH_GUI === '1';
const MAC = process.platform === 'darwin';

if (!BIN || !fs.existsSync(BIN)) {
  console.log('skip: set DESKHATCH_BIN to a built runtime');
  process.exit(0);
}

let home, bundle, failures = 0, ran = 0, skipped = 0;

/// A standalone app is a copy of the runtime's own .app bundle, so those cases need the binary
/// to live inside one. Rather than ask CI for a full `tauri build`, wrap the binary in the
/// smallest bundle that macOS and the runtime both accept.
function makeBundle() {
  if (BUNDLE) return BUNDLE;
  if (!MAC) return null;
  const b = path.join(home, 'runtime', 'Deskhatch.app');
  fs.mkdirSync(path.join(b, 'Contents', 'MacOS'), { recursive: true });
  fs.mkdirSync(path.join(b, 'Contents', 'Resources'), { recursive: true });
  fs.writeFileSync(path.join(b, 'Contents', 'Info.plist'), [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">',
    '<plist version="1.0"><dict>',
    '<key>CFBundleName</key><string>Deskhatch</string>',
    '<key>CFBundleIdentifier</key><string>tech.besk.deskhatch.test</string>',
    '<key>CFBundleExecutable</key><string>deskhatch</string>',
    '<key>CFBundlePackageType</key><string>APPL</string>',
    '<key>CFBundleVersion</key><string>0.0.0</string>',
    '</dict></plist>',
  ].join('\n'));
  fs.copyFileSync(BIN, path.join(b, 'Contents', 'MacOS', 'deskhatch'));
  fs.chmodSync(path.join(b, 'Contents', 'MacOS', 'deskhatch'), 0o755);
  spawnSync('codesign', ['--force', '--deep', '--sign', '-', b]);
  return b;
}
function run() {
  return bundle ? path.join(bundle, 'Contents', 'MacOS', 'deskhatch') : BIN;
}

function sh(args, opts = {}) {
  return spawnSync(run(), args, {
    encoding: 'utf8',
    env: { ...process.env, DESKHATCH_HOME: home, HOME: home, ...(opts.env || {}) },
    timeout: opts.timeout || 90000,
    input: opts.input,
  });
}
function ok(r, what) {
  if (r.status !== 0) throw new Error(`${what}: exit ${r.status}\n${r.stdout}\n${r.stderr}`);
  return r.stdout;
}
function pkg(dir, manifest, { icon = false } = {}) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'app.json'), JSON.stringify(manifest));
  fs.writeFileSync(path.join(dir, 'index.html'), `<!doctype html><title>${manifest.name}</title><h1>x</h1>`);
  if (icon) fs.writeFileSync(path.join(dir, 'icon.png'), Buffer.alloc(0));
  return dir;
}
function plist(bundle) {
  const r = spawnSync('plutil', ['-convert', 'json', '-o', '-', path.join(bundle, 'Contents', 'Info.plist')], { encoding: 'utf8' });
  if (r.status !== 0) throw new Error(`plutil failed for ${bundle}: ${r.stderr}`);
  return JSON.parse(r.stdout);
}
function sleep(ms) {
  // Deliberately synchronous: these tests are a straight line, not a pipeline.
  spawnSync(process.execPath, ['-e', `setTimeout(()=>{}, ${ms})`]);
}

const cases = [];
function test(name, fn, need) { cases.push({ name, fn, need }); }

// ---------------------------------------------------------------- package import

test('a package folder without an icon still gets one', () => {
  pkg(path.join(home, 'src-noicon'), { schema: 1, id: 'noicon', name: 'No Icon', version: '1.0.0', permissions: 'web' });
  const out = ok(sh(['install', path.join(home, 'src-noicon'), '--no-launcher']), 'install');
  if (!out.includes('noicon')) throw new Error(`unexpected output: ${out}`);
  const icon = path.join(home, 'apps', 'noicon', 'icon.png');
  if (!fs.existsSync(icon)) throw new Error('icon.png was not drawn');
  if (fs.statSync(icon).size < 100) throw new Error('icon.png is empty');
});

test('a .hatch without an icon still gets one', () => {
  pkg(path.join(home, 'src-hatch'), { schema: 1, id: 'hatchless', name: 'Hatchless', version: '1.0.0', permissions: 'web' });
  ok(sh(['install', path.join(home, 'src-hatch'), '--no-launcher']), 'install');
  ok(sh(['export', 'hatchless', '--out', home]), 'export');
  ok(sh(['uninstall', 'hatchless']), 'uninstall');
  fs.rmSync(path.join(home, 'apps', 'hatchless'), { recursive: true, force: true });
  ok(sh(['install', path.join(home, 'hatchless.hatch'), '--no-launcher']), 'install .hatch');
  if (!fs.existsSync(path.join(home, 'apps', 'hatchless', 'icon.png'))) throw new Error('icon.png was not drawn');
});

// ---------------------------------------------------------------- stdout and pipes

test('a closed pipe does not panic', () => {
  for (let i = 0; i < 400; i++) {
    const id = `big${String(i).padStart(4, '0')}`;
    const dir = path.join(home, 'apps', id);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'app.json'), JSON.stringify({ schema: 1, id, name: `Application number ${id} with a deliberately long name`, version: '1.0.0', permissions: 'web' }));
    fs.writeFileSync(path.join(dir, 'index.html'), '<!doctype html><title>x</title>');
  }
  const r = spawnSync('/bin/sh', ['-c', `"$0" list 2>"$1" | head -3`, run(), path.join(home, 'pipe.err')], {
    encoding: 'utf8',
    env: { ...process.env, DESKHATCH_HOME: home, HOME: home },
    timeout: 90000,
  });
  const err = fs.readFileSync(path.join(home, 'pipe.err'), 'utf8');
  if (/panicked|Broken pipe/i.test(err)) throw new Error(`panicked on a closed pipe:\n${err}`);
  if (r.stdout.split('\n').filter(Boolean).length !== 3) throw new Error(`head -3 got ${r.stdout}`);
  const full = ok(sh(['list']), 'list');
  if (full.split('\n').filter(Boolean).length < 400) throw new Error('full output is truncated');
  for (let i = 0; i < 400; i++) fs.rmSync(path.join(home, 'apps', `big${String(i).padStart(4, '0')}`), { recursive: true, force: true });
});

test('config survives head -3', () => {
  const r = spawnSync('/bin/sh', ['-c', `"$0" config 2>"$1" | head -3`, run(), path.join(home, 'config.err')], {
    encoding: 'utf8', env: { ...process.env, DESKHATCH_HOME: home, HOME: home }, timeout: 60000,
  });
  const err = fs.readFileSync(path.join(home, 'config.err'), 'utf8');
  if (/panicked/i.test(err)) throw new Error(`panicked:\n${err}`);
  if (!r.stdout.includes('provider')) throw new Error(`unexpected output: ${r.stdout}`);
});

// ---------------------------------------------------------------- launchers

// Every later case uses thinapp, so this one must run on every platform.
test('the launcher carries the app name and points at the runtime', () => {
  pkg(path.join(home, 'src-thin'), { schema: 1, id: 'thinapp', name: 'Thin App', version: '2.1.0', permissions: 'web' });
  const out = ok(sh(['install', path.join(home, 'src-thin')]), 'install');
  if (!/launcher/.test(out)) throw new Error(`no launcher in output: ${out}`);
  if (MAC) {
    const bundle = path.join(home, 'Applications', 'Thin App.app');
    if (!fs.existsSync(bundle)) throw new Error('launcher bundle missing');
    const p = plist(bundle);
    if (p.CFBundleName !== 'Thin App') throw new Error(`CFBundleName is ${p.CFBundleName}`);
    if (p.CFBundleIdentifier !== 'tech.besk.deskhatch.app.thinapp') throw new Error(`id is ${p.CFBundleIdentifier}`);
    const script = fs.readFileSync(path.join(bundle, 'Contents', 'MacOS', 'launch'), 'utf8');
    if (!script.includes('--app') || !script.includes('thinapp')) throw new Error(`launcher script: ${script}`);
  } else {
    const entry = path.join(home, '.local', 'share', 'applications', 'deskhatch-thinapp.desktop');
    if (!fs.existsSync(entry)) throw new Error('desktop entry missing');
    const d = fs.readFileSync(entry, 'utf8');
    if (!/^Name=Thin App$/m.test(d)) throw new Error(`desktop entry has no name:\n${d}`);
    if (!/--app thinapp/.test(d)) throw new Error(`desktop entry does not start the app:\n${d}`);
  }
});

test('a standalone bundle gets its own name, identifier, icon and app id', () => {
  pkg(path.join(home, 'src-solo'), { schema: 1, id: 'soloapp', name: 'Solo App', version: '3.4.5', permissions: 'web' });
  const out = ok(sh(['install', path.join(home, 'src-solo'), '--standalone']), 'install --standalone');
  if (!out.includes('(standalone)')) throw new Error(`not reported as standalone: ${out}`);
  const bundle = path.join(home, 'Applications', 'Solo App.app');
  const p = plist(bundle);
  if (p.CFBundleName !== 'Solo App') throw new Error(`CFBundleName is ${p.CFBundleName}`);
  if (p.CFBundleIdentifier !== 'tech.besk.deskhatch.app.soloapp') throw new Error(`id is ${p.CFBundleIdentifier}`);
  if (p.CFBundleExecutable !== 'deskhatch') throw new Error(`executable is ${p.CFBundleExecutable}`);
  if (p.CFBundleShortVersionString !== '3.4.5') throw new Error(`version is ${p.CFBundleShortVersionString}`);
  const marker = path.join(bundle, 'Contents', 'Resources', 'deskhatch-app');
  if (fs.readFileSync(marker, 'utf8').trim() !== 'soloapp') throw new Error('app id marker is wrong');
  if (!fs.existsSync(path.join(bundle, 'Contents', 'Resources', 'app.icns'))) throw new Error('app.icns missing');
  const sign = spawnSync('codesign', ['-dv', bundle], { encoding: 'utf8' });
  if (!/adhoc/.test(sign.stderr)) throw new Error(`copy is not ad-hoc signed:\n${sign.stderr}`);
  // No .hatch association and no leftover staging or backup bundles.
  if (p.CFBundleDocumentTypes) throw new Error('the copy still claims .hatch files');
  for (const leftover of ['Solo App.app.new', 'Solo App.app.old']) {
    if (fs.existsSync(path.join(home, 'Applications', leftover))) throw new Error(`${leftover} was left behind`);
  }
}, MAC);

// Without this there is no way to open the gallery except a terminal.
test('deskhatch itself gets an icon next to the apps', () => {
  if (MAC) {
    const bundle = path.join(home, 'Applications', 'deskhatch.app');
    if (!fs.existsSync(bundle)) throw new Error('deskhatch.app was not created');
    const p = plist(bundle);
    if (p.CFBundleName !== 'deskhatch') throw new Error(`CFBundleName is ${p.CFBundleName}`);
    if (p.CFBundleIdentifier !== 'tech.besk.deskhatch.gallery') throw new Error(`id is ${p.CFBundleIdentifier}`);
    const script = fs.readFileSync(path.join(bundle, 'Contents', 'MacOS', 'launch'), 'utf8');
    if (/--app/.test(script)) throw new Error(`the gallery launcher starts an app: ${script}`);
  } else {
    const entry = path.join(home, '.local', 'share', 'applications', 'deskhatch.desktop');
    if (!fs.existsSync(entry)) throw new Error('deskhatch.desktop was not created');
    const d = fs.readFileSync(entry, 'utf8');
    if (/--app/.test(d)) throw new Error(`the gallery entry starts an app:\n${d}`);
  }
});

test('relink keeps each launcher in the mode it had', () => {
  const out = ok(sh(['relink']), 'relink');
  if (!/^deskhatch\s/m.test(out)) throw new Error(`relink did not rebuild deskhatch's own icon: ${out}`);
  if (!/thinapp\s/.test(out)) throw new Error(`thin app missing from relink: ${out}`);
  if (MAC && !/soloapp.*\(standalone\)/.test(out)) throw new Error(`standalone not preserved: ${out}`);
  if (/thinapp.*\(standalone\)/.test(out)) throw new Error(`thin app was upgraded behind our back: ${out}`);
  const thin = MAC
    ? path.join(home, 'Applications', 'Thin App.app', 'Contents', 'MacOS', 'launch')
    : path.join(home, '.local', 'share', 'applications', 'deskhatch-thinapp.desktop');
  if (!fs.existsSync(thin)) throw new Error('the thin launcher did not survive relink');
  if (MAC) {
    const marker = path.join(home, 'Applications', 'Solo App.app', 'Contents', 'Resources', 'deskhatch-app');
    if (!fs.existsSync(marker)) throw new Error('standalone bundle lost its app id');
  }
});

test('--name renames a package that brought its own manifest', () => {
  pkg(path.join(home, 'src-named'), { schema: 1, id: 'namedapp', name: 'Original', version: '1.0.0', permissions: 'web' });
  ok(sh(['install', path.join(home, 'src-named'), '--name', 'Renamed', '--no-launcher']), 'install --name');
  const m = JSON.parse(fs.readFileSync(path.join(home, 'apps', 'namedapp', 'app.json'), 'utf8'));
  if (m.name !== 'Renamed') throw new Error(`the installed manifest still says ${m.name}`);
  if (m.id !== 'namedapp') throw new Error(`the id changed to ${m.id}`);
  const list = ok(sh(['list']), 'list');
  if (!/Renamed/.test(list)) throw new Error(`list does not show the new name: ${list}`);
  ok(sh(['uninstall', 'namedapp']), 'uninstall');
});

// ---------------------------------------------------------------- windows and the gallery

test('a standalone bundle hosts its own app with no arguments', () => {
  // No arguments at all: the bundle must find its app id in its own Resources.
  const copied = path.join(home, 'Applications', 'Solo App.app', 'Contents', 'MacOS', 'deskhatch');
  const r2 = spawnSync(copied, ['--eval', 'hatch.ready.then(a => a.id)'], {
    encoding: 'utf8',
    env: { ...process.env, DESKHATCH_HOME: home, HOME: home, DESKHATCH_TEST: '1' },
    timeout: 60000,
  });
  const out = (r2.stdout || '').trim();
  if (out !== '"soloapp"') throw new Error(`the bundle hosted ${out || r2.stderr} instead of soloapp`);
}, MAC && GUI);

test('the gallery can recreate a launcher and keeps its mode', () => {
  const out = evalInGallery('hatch._gallery.launcher("soloapp")');
  if (!out.includes('Solo App.app')) throw new Error(`recreate returned ${out}`);
  const marker = path.join(home, 'Applications', 'Solo App.app', 'Contents', 'Resources', 'deskhatch-app');
  if (!fs.existsSync(marker)) throw new Error('recreate downgraded a standalone app to a thin launcher');
}, MAC && GUI);

test('the gallery does not claim to open an app that is gone', () => {
  const out = evalInGallery('hatch._gallery.open("no-such-app").then(() => "CLAIMED OK").catch(e => "refused: " + e)');
  if (!/refused/.test(out)) throw new Error(`the Open button reported success for a missing app: ${out}`);
});

test('the gallery lists what is installed', () => {
  const out = evalInGallery('hatch._gallery.list().then(l => l.map(a => a.id).sort())');
  if (!out.includes('thinapp')) throw new Error(`gallery list: ${out}`);
}, GUI);

test('one app, one process: a second start hands the window over', () => {
  // The whole suite is synchronous, so the child's stderr goes to a file rather than to a
  // stream callback that would never get a turn on the event loop.
  const log = fs.openSync(path.join(home, 'owner.err'), 'w');
  const child = spawn(run(), ['--app', 'thinapp'], {
    env: { ...process.env, DESKHATCH_HOME: home, HOME: home },
    stdio: ['ignore', 'ignore', log],
  });
  try {
    sleep(4000);
    const second = sh(['--app', 'thinapp'], { timeout: 30000 });
    if (second.status !== 0) throw new Error(`the second start failed: ${second.status} ${second.stderr}`);
    sleep(2000);
    if (child.exitCode !== null) throw new Error('the first instance died instead of staying up');
    const err = fs.readFileSync(path.join(home, 'owner.err'), 'utf8');
    if (!/bringing it forward/.test(err)) throw new Error(`the owner never saw the handover:\n${err}`);
    // The newcomer must say what happened, or the command looks like it did nothing.
    if (!/already open, brought its window forward/.test(second.stderr)) {
      throw new Error(`the second start said nothing: ${second.stderr}`);
    }
  } finally {
    child.kill();
    fs.closeSync(log);
    sleep(500);
  }
}, GUI);

function evalInGallery(expr) {
  const r = spawnSync(run(), ['--eval', expr], {
    encoding: 'utf8',
    env: { ...process.env, DESKHATCH_HOME: home, HOME: home, DESKHATCH_TEST: '1' },
    timeout: 60000,
  });
  const out = (r.stdout || '').trim();
  if (!out) throw new Error(`the gallery printed nothing (exit ${r.status})\n${r.stderr}`);
  if (out.startsWith('"ERROR') || out.startsWith('ERROR')) throw new Error(out);
  return out;
}

// ---------------------------------------------------------------- runner

home = fs.mkdtempSync(path.join(os.tmpdir(), 'dh-test-'));
fs.mkdirSync(path.join(home, 'Applications'), { recursive: true });
bundle = makeBundle();
process.on('exit', () => { try { fs.rmSync(home, { recursive: true, force: true }); } catch {} });

for (const c of cases) {
  if (c.need === false) { skipped++; console.log(`skip  ${c.name}`); continue; }
  ran++;
  try {
    c.fn();
    console.log(`ok    ${c.name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL  ${c.name}\n      ${String(e.message).split('\n').join('\n      ')}`);
  }
}
console.log(`\n${ran - failures}/${ran} passed, ${skipped} skipped`);
process.exit(failures ? 1 : 0);
