// Minimal CLI test: forwards to a runtime binary given by DESKHATCH_BIN.
'use strict';
const { spawnSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

const bin = process.env.DESKHATCH_BIN;
if (!bin || !fs.existsSync(bin)) {
  console.log('skip: set DESKHATCH_BIN to a built runtime to run the CLI test');
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
console.log('cli test ok');
