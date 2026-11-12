import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { recordInstaller, stageSidecars, target } from './package-windows.mjs';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'cafe-bundle-fixture-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = join(root, 'source'); mkdirSync(source);
  return { source, destination: join(root, 'output') };
}

test('both release executables are copied byte-for-byte with the Tauri target suffix', t => {
  const { source, destination } = fixture(t);
  for (const name of ['cafe-spaced', 'cafe-space']) writeFileSync(join(source, `${name}.exe`), name);
  stageSidecars(source, destination);
  assert.deepEqual(readdirSync(destination).sort(), [`cafe-space-${target}.exe`, `cafe-spaced-${target}.exe`]);
  for (const name of ['cafe-spaced', 'cafe-space']) assert.equal(readFileSync(join(destination, `${name}-${target}.exe`), 'utf8'), name);
});

test('an absent CLI prevents packaging instead of shipping a daemon-only app', t => {
  const { source, destination } = fixture(t);
  writeFileSync(join(source, 'cafe-spaced.exe'), 'daemon');
  assert.throws(() => stageSidecars(source, destination));
});

test('the uploaded installer hash and manifest identify exactly one commit', t => {
  const { source, destination } = fixture(t);
  const commit = 'a'.repeat(40), bytes = Buffer.from('fixture installer');
  writeFileSync(join(source, 'Café Space_0.1.0_x64-setup.exe'), bytes);
  const manifest = recordInstaller(source, destination, commit);
  const hash = createHash('sha256').update(bytes).digest('hex');
  assert.equal(manifest.commit, commit);
  assert.equal(manifest.target, target);
  assert.equal(manifest.unsigned, true);
  assert.equal(manifest.artifacts[0].sha256, hash);
  assert.equal(readFileSync(join(destination, 'SHA256SUMS.txt'), 'utf8'), `${hash}  Café Space_0.1.0_x64-setup.exe\n`);
  assert.deepEqual(JSON.parse(readFileSync(join(destination, 'manifest.json'), 'utf8')), manifest);
  assert.throws(() => recordInstaller(source, destination, commit), /must be empty/);
});

test('missing or ambiguous installer outputs are rejected', t => {
  const { source, destination } = fixture(t);
  assert.throws(() => recordInstaller(source, destination, 'a'.repeat(40)), /exactly one/);
  writeFileSync(join(source, 'old.exe'), 'old'); writeFileSync(join(source, 'new.exe'), 'new');
  assert.throws(() => recordInstaller(source, destination, 'a'.repeat(40)), /exactly one/);
});

test('packaging is opt-in, current-user NSIS only, and never grants WebView shell permissions', () => {
  const config = JSON.parse(readFileSync(new URL('../src-tauri/tauri.windows-bundle.conf.json', import.meta.url)));
  assert.equal(config.bundle.active, true);
  assert.deepEqual(config.bundle.targets, ['nsis']);
  assert.deepEqual(config.bundle.externalBin, ['binaries/cafe-spaced', 'binaries/cafe-space']);
  assert.deepEqual(config.bundle.windows.nsis, { installMode: 'currentUser', installerHooks: 'windows/hooks.nsh' });
  assert.equal(config.app, undefined);
  assert.equal(config.bundle.windows.signCommand, undefined);
  const cargo = readFileSync(new URL('../src-tauri/Cargo.toml', import.meta.url), 'utf8');
  assert.match(cargo, /tauri-build = \{ version = "=2\.6\.3", features = \[\] \}/, 'Avoid Tauri CLI rewriting the source checkout');
  const hook = readFileSync(new URL('../src-tauri/windows/hooks.nsh', import.meta.url), 'utf8');
  assert.match(hook, /\$EXEDIR != \$INSTDIR[\s\S]*\$UpdateMode != 1[\s\S]*ExecWait/);
  assert.match(hook, /"\$INSTDIR\\cafe-space.exe" autostart uninstall/);
  assert.match(hook, /\$0 != 0[\s\S]*MessageBox[^\r\n]*无法停止 Café Space 守护进程，已取消卸载[\s\S]*SetErrorLevel 1[\s\S]*Abort/);
  assert.doesNotMatch(hook, /could not verify|Nothing else will be removed|\bClose the app\b|RMDir|DeleteReg|taskkill|schtasks/i);
});
