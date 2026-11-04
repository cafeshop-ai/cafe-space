// Explicit, disposable upstream-driver comparison; never changes checked-in Cargo manifests/locks.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {cp,mkdtemp,readFile,writeFile,appendFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const here=path.dirname(fileURLToPath(import.meta.url));
export const originalDriverHash='0edb347b10805852911709b15ce21af2682f9f0580765834ca2b3b1adb7b8292';
const hash=text=>createHash('sha256').update(text).digest('hex');
export function patchDriver(original) {
  const begin='        // 1.b turn_relayer poll_write()',next='        // 1.c peer_connection poll_write()';
  assert.equal(original.split(begin).length,2);assert.equal(original.split(next).length,2);
  const start=original.indexOf(begin),end=original.indexOf(next,start);
  assert(start>=0 && end>start);
  const turn=original.slice(start,end);
  const tail='        self.flush_writes(writes).await;\n\n        Ok(())\n    }';
  assert.equal(original.split(tail).length,2);
  // Keep the original early drain; add only a second drain after core output has been routed.
  return original.slice(0,start)+'        self.flush_turn_writes().await;\n\n'+original.slice(end).replace(tail,
    '        self.flush_writes(writes).await;\n        self.flush_turn_writes().await;\n\n        Ok(())\n    }\n\n    async fn flush_turn_writes(&mut self) {\n'+turn.trimEnd()+'\n    }');
}
export async function buildDriverProbe() {
  const originalLock=await readFile(path.join(here,'Cargo.lock'));
  const metadata=JSON.parse(execFileSync('cargo',['metadata','--locked','--format-version','1','--manifest-path',path.join(here,'Cargo.toml')],{encoding:'utf8',maxBuffer:16*1024*1024,timeout:120000}));
  const dependency=metadata.packages.find(pkg=>pkg.name==='webrtc' && pkg.version==='0.21.1');
  assert(dependency?.source?.startsWith('registry+'),'must copy the locked registry release, not a preexisting fork');
  const directory=await mkdtemp(path.join(process.env.RUNNER_TEMP || tmpdir(),'cafe-space-r4-driver-'));
  const fork=path.join(directory,'webrtc'),workspace=path.join(directory,'spike');
  await cp(path.dirname(dependency.manifest_path),fork,{recursive:true});
  for(const name of ['Cargo.toml','Cargo.lock','rust-toolchain.toml','src'])await cp(path.join(here,name),path.join(workspace,name),{recursive:true});
  const driver=path.join(fork,'src/peer_connection/driver.rs');
  const original=await readFile(driver,'utf8');
  assert.equal(hash(original),originalDriverHash,'unexpected upstream driver; do not apply a guessed patch');
  const patched=patchDriver(original);
  await writeFile(driver,patched);
  const manifest=path.join(workspace,'Cargo.toml');
  const config='patch.crates-io.webrtc.path='+JSON.stringify(fork);
  const cargo=(command,extra=[])=>execFileSync('cargo',[command,'--offline','--manifest-path',manifest,'--config',config,...extra],{stdio:'inherit',timeout:600000,env:{...process.env,CARGO_TARGET_DIR:path.join(workspace,'target')}});
  // Only the disposable copy's lock changes from registry source to this explicit path patch.
  cargo('test');cargo('clippy',['--all-targets','--','-D','warnings']);cargo('build');
  assert((await readFile(path.join(here,'Cargo.lock'))).equals(originalLock),'original lock must stay byte-identical');
  const binary=path.join(workspace,'target/debug/cafe-space-webrtc-spike'+(process.platform==='win32'?'.exe':''));
  const result={variant:'turn-drain',upstream:'webrtc 0.21.1',originalDriverHash,patchedDriverHash:hash(patched),binary};
  await writeFile(path.join(directory,'probe.json'),JSON.stringify(result,null,2));
  return result;
}
if(process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  assert.equal(process.env.CAFE_SPACE_TEST_R4,'1');
  assert.equal(process.env.RUNNER_ENVIRONMENT,'github-hosted');
  assert.equal(process.env.CAFE_SPACE_R4_DRIVER_VARIANT,'turn-drain');
  assert(process.env.GITHUB_ENV);
  const result=await buildDriverProbe();
  await appendFile(process.env.GITHUB_ENV,'CAFE_SPACE_R4_DRIVER_BINARY='+result.binary+'\n');
  const {binary,...publicResult}=result;
  console.log(JSON.stringify({event:'R4_DRIVER_PROBE_BUILD',...publicResult,acceptance:false}));
}
