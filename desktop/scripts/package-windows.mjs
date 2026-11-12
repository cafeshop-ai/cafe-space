import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const target = 'x86_64-pc-windows-msvc';
export const versions = { rust: '1.95.0', node: '22', pnpm: '10.30.3', tauri: '2.8.4' };
const root = fileURLToPath(new URL('../../', import.meta.url));
const desktop = join(root, 'desktop/src-tauri');
const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex');

export function stageSidecars(source, destination) {
  mkdirSync(destination, { recursive: true });
  assert(!lstatSync(destination).isSymbolicLink(), 'Sidecar directory must not be a link');
  for (const name of ['cafe-spaced', 'cafe-space']) {
    const input = join(source, `${name}.exe`);
    const output = join(destination, `${name}-${target}.exe`);
    assert(lstatSync(input).isFile() && !lstatSync(input).isSymbolicLink() && lstatSync(input).size > 0, 'Missing release executable');
    if (existsSync(output)) assert(lstatSync(output).isFile() && !lstatSync(output).isSymbolicLink(), 'Invalid sidecar destination');
    copyFileSync(input, output);
    assert.equal(digest(output), digest(input), 'Sidecar copy differs');
  }
}

export function recordInstaller(source, destination, commit) {
  assert.match(commit, /^[a-f0-9]{40}$/);
  const installers = readdirSync(source).filter(name => name.endsWith('.exe'));
  assert.equal(installers.length, 1, 'Expected exactly one NSIS installer; inspect stale outputs');
  const name = installers[0], input = join(source, name);
  assert(lstatSync(input).isFile() && !lstatSync(input).isSymbolicLink());
  mkdirSync(destination, { recursive: true });
  assert(!lstatSync(destination).isSymbolicLink());
  // Refuse stale output rather than silently uploading installers from different commits.
  assert.equal(readdirSync(destination).length, 0, 'Artifact directory must be empty');
  const output = join(destination, name);
  copyFileSync(input, output);
  const sha256 = digest(input);
  assert.equal(digest(output), sha256);
  const manifest = { commit, target, unsigned: true, versions, artifacts: [{ file: name, sha256 }] };
  writeFileSync(join(destination, 'SHA256SUMS.txt'), `${sha256}  ${name}\n`);
  writeFileSync(join(destination, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
  return manifest;
}

export function main() {
  assert.equal(process.platform, 'win32', 'Use native Windows tooling');
  assert.equal(process.arch, 'x64', 'Use native Windows x64 Node');
  assert.equal(process.versions.node.split('.')[0], versions.node, 'Use Node 22');
  const env = { ...process.env, CARGO_BUILD_TARGET: target };
  function run(program, args, cwd = root, capture = false) {
    return execFileSync(program, args, { cwd, env, stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit', ...(capture ? { encoding: 'utf8' } : {}) });
  }
  // A .cmd shim needs cmd.exe; only fixed pnpm commands enter that shell, never paths or user input.
  function pnpm(args, cwd, capture = false) {
    assert(args.every(arg => /^[a-z0-9.-]+$/.test(arg)));
    const system = process.env.SystemRoot;
    assert(system && resolve(system) === system);
    return run(join(system, 'System32/cmd.exe'), ['/d', '/s', '/c', `pnpm.cmd ${args.join(' ')}`], cwd, capture);
  }
  assert.equal(run('git', ['status', '--porcelain'], root, true).trim(), '', 'Commit all changes before packaging');
  const commit = run('git', ['rev-parse', 'HEAD'], root, true).trim();
  assert.match(commit, /^[a-f0-9]{40}$/);
  assert.equal(run('rustc', ['--version'], root, true).split(' ')[1], versions.rust);
  assert.equal(pnpm(['--version'], root, true).trim(), versions.pnpm);
  assert.equal(run('cargo', ['tauri', '--version'], root, true).trim(), `tauri-cli ${versions.tauri}`);
  const output = join(root, 'target/windows-bundle');
  if (existsSync(output)) assert.equal(readdirSync(output).length, 0, 'Artifact directory must be empty');
  const metadata = cwd => JSON.parse(run('cargo', ['metadata', '--offline', '--locked', '--no-deps', '--format-version', '1'], cwd, true));
  const artifacts = join(metadata(root).target_directory, target, 'release');
  const bundle = join(metadata(desktop).target_directory, target, 'release/bundle/nsis');
  if (existsSync(bundle)) assert(!readdirSync(bundle).some(name => name.endsWith('.exe')), 'Inspect previous NSIS output before packaging again');
  pnpm(['install', '--frozen-lockfile'], join(root, 'web'));
  pnpm(['build'], join(root, 'web'));
  run('cargo', ['build', '--locked', '--release', '--target', target, '-p', 'cafe-spaced', '--bin', 'cafe-spaced', '-p', 'cafe-space-cli', '--bin', 'cafe-space']);
  stageSidecars(artifacts, join(desktop, 'binaries'));
  run('cargo', ['tauri', 'build', '--target', target, '--bundles', 'nsis', '--config', 'tauri.windows-bundle.conf.json', '--', '--locked'], desktop);
  assert.equal(run('git', ['rev-parse', 'HEAD'], root, true).trim(), commit, 'Commit changed during build');
  assert.equal(run('git', ['status', '--porcelain'], root, true).trim(), '', 'Source changed during build');
  const manifest = recordInstaller(bundle, output, commit);
  console.log(JSON.stringify({ directory: output, ...manifest }));
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
