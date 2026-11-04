// Disposable CI only. No Café identity, production TURN, or host firewall changes.
import assert from 'node:assert/strict';
import {spawn,execFileSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {mkdtemp,writeFile,readFile,rm,stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import net from 'node:net';
import dgram from 'node:dgram';
import {randomBytes,createHash} from 'node:crypto';
import {startLab} from './lab.mjs';
import {macReceiveBufferDrops} from './udp-stats.mjs';
import {webdriver,driverMilestone} from './webdriver.mjs';
import {turnStats} from './turn-stats.mjs';
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const browser=process.argv[2];
assert.equal(process.env.CAFE_SPACE_TEST_R4,'1');
assert.equal(process.env.RUNNER_ENVIRONMENT,'github-hosted');
assert(/^[a-f0-9]{40}$/.test(process.env.GITHUB_SHA || ''));
assert(['chrome','firefox','safari'].includes(browser));
const netns=process.env.CAFE_SPACE_R4_NETNS==='1';
assert(netns || (['safari','chrome'].includes(browser) && process.platform==='darwin'));
if(netns) {
  assert.equal(process.platform,'linux');assert(process.getuid()>0);
  assert.equal((await stat('/proc/self/ns/net')).ino,(await stat('/var/run/netns/r4-lab')).ino,'must run in the new lab namespace');
}
const ip=netns?'10.203.0.1':'127.0.0.1';
const nsPrefix=['-n','ip','netns','exec','r4-rust'];
const userPrefix=['sudo','-n','-u','#'+process.getuid?.(),'--'];
const nft=(...args)=>execFileSync('sudo',[...nsPrefix,'nft',...args],{encoding:'utf8',stdio:['pipe','pipe','pipe']});
function udpErrors(){
  try{
    if(process.platform==='darwin'){
      const output=execFileSync('/usr/sbin/netstat',['-s','-p','udp'],{encoding:'utf8',stdio:['ignore','pipe','pipe']});
      return {scope:'runner-global-not-socket-specific',receiveBufferDrops:macReceiveBufferDrops(output)};
    }
    if(!netns)return {available:false};
    return Object.fromEntries(['r4-lab','r4-rust'].map(namespace=>{
    const output=execFileSync('sudo',['-n','ip','netns','exec',namespace,'nstat','-asz','UdpInErrors','UdpRcvbufErrors','UdpSndbufErrors'],{encoding:'utf8',stdio:['ignore','pipe','pipe']});
    const counters=Object.fromEntries([...output.matchAll(/^(UdpInErrors|UdpRcvbufErrors|UdpSndbufErrors)\s+(\d+)/gm)].map(([,key,value])=>[key,Number(value)]));
    assert.equal(Object.keys(counters).length,3);
    return [namespace,counters];
  }));}catch(error){return {available:false,reason:['ERR_ASSERTION','ENOENT','ETIMEDOUT'].includes(error.code)?error.code:'COUNTER_READ_FAILED'};}
}
async function freePort(){const server=net.createServer();await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const port=server.address().port;await new Promise(resolve=>server.close(resolve));return port;}
async function waitFor(check,ms,code){const until=Date.now()+ms;while(!await check()){if(Date.now()>=until)throw Error(code);await sleep(100);}}
async function stopChild(child){if(!child || child.exitCode!==null || child.signalCode!==null)return;child.kill();await Promise.race([new Promise(resolve=>child.once('exit',resolve)),sleep(3000)]);if(child.exitCode===null && child.signalCode===null)child.kill('SIGKILL');}
async function networkProbe(blocked){
  let inbound=0;
  const udp=dgram.createSocket('udp4');udp.on('message',data=>{if(data.toString()==='R4_UDP')inbound++;});
  await new Promise(resolve=>udp.bind(0,ip,resolve));
  const tcp=net.createServer(socket=>{socket.setTimeout(1000,()=>socket.destroy());socket.once('data',data=>socket.end(data.equals(Buffer.from('R4_TCP'))?data:Buffer.alloc(0)));});
  await new Promise(resolve=>tcp.listen(0,ip,resolve));
  const script=`const dgram=require('node:dgram'),net=require('node:net');let udpIn=0,tcpOk=false;const u=dgram.createSocket('udp4');u.on('message',m=>{if(m.toString()==='R4_UDP')udpIn++});u.bind(0,'10.203.0.2',()=>{console.log(JSON.stringify({port:u.address().port}));u.send('R4_UDP',Number(process.argv[1]),'10.203.0.1')});const t=net.connect(Number(process.argv[2]),'10.203.0.1',()=>t.write('R4_TCP'));t.on('data',m=>{tcpOk=m.toString()==='R4_TCP';t.end()});t.on('error',()=>{});setTimeout(()=>{console.log(JSON.stringify({udpIn,tcpOk}));u.close();t.destroy()},1000);`;
  const child=spawn('sudo',[...nsPrefix,...userPrefix,process.execPath,'-e',script,String(udp.address().port),String(tcp.address().port)],{stdio:['ignore','pipe','ignore']});
  const lines=createInterface({input:child.stdout});let result;
  lines.on('line',line=>{const value=JSON.parse(line);if(value.port)udp.send('R4_UDP',value.port,'10.203.0.2');else result=value;});
  try {
    await waitFor(()=>child.exitCode!==null || child.signalCode!==null,5000,'NETWORK_PROBE_TIMEOUT');
    assert.equal(child.exitCode,0);assert(result?.tcpOk,'TCP must still work');
    assert.equal(inbound,blocked?0:1);assert.equal(result.udpIn,blocked?0:1);
    return {udpToLab:inbound,udpToRust:result.udpIn,tcp:true};
  } finally {lines.close();await stopChild(child);udp.close();await new Promise(resolve=>tcp.close(resolve));}
}
const directory=await mkdtemp(path.join(tmpdir(),'cafe-space-r4-'));
const username=randomBytes(16).toString('hex'),credential=randomBytes(32).toString('hex');
const servers=[{urls:['turn:'+ip+':3478?transport=udp'],username,credential}];
const config=path.join(directory,'turn.conf');
await writeFile(config,[`listening-ip=${ip}`,`relay-ip=${ip}`,'listening-port=3478','min-port=49160','max-port=49215','fingerprint','lt-cred-mech','realm=r4.invalid',`user=${username}:${credential}`,'user-quota=8','total-quota=16','no-tcp','no-tls','no-dtls','no-tcp-relay','no-multicast-peers','no-stdout-log','simple-log','verbose',`log-file=${path.join(directory,'turn.log')}`,`pidfile=${path.join(directory,'turn.pid')}`,...(netns?[]:['allow-loopback-peers'])].join('\n')+'\n',{mode:0o600});
let turn,driver,lab,base,session,turnVersion,browserVersion,blocked=false,phase='coturn_start';
const results=[],fixtureFailures=[],driverLines=[],driverMilestones=new Set();
const repetitions=10;
const required={K_browser_pair_relay:repetitions,A_direct:repetitions,B_udp_relay:repetitions,B1_browser_relay:repetitions,B2_rust_relay:repetitions,...(netns?{C_computer_udp_blocked:1,D_restored_relay:repetitions}:{})};
try {
  turn=spawn('turnserver',['-c',config],{stdio:'ignore'});turn.on('error',()=>{});
  await sleep(1000);assert.equal(turn.exitCode,null);assert(turn.pid,'coturn must start');
  turnVersion=(await readFile(path.join(directory,'turn.log'),'utf8')).match(/Version\s+Coturn-([0-9][0-9A-Za-z.+-]{0,63})/)?.[1];
  assert(turnVersion,'record the actual coturn version');
  phase='webdriver_start';
  const port=await freePort();base='http://127.0.0.1:'+port;
  let executable,args;
  if(browser==='chrome') {executable=process.env.CHROMEWEBDRIVER?path.join(process.env.CHROMEWEBDRIVER,'chromedriver'):'chromedriver';args=['--port='+port,'--allowed-ips=127.0.0.1'];}
  if(browser==='firefox') {executable='geckodriver';args=['--host','127.0.0.1','--port',String(port)];}
  if(browser==='safari') {executable='/usr/bin/safaridriver';args=['--port',String(port)];}
  driver=spawn(executable,args,{stdio:['ignore','pipe','pipe']});driver.on('error',()=>{});
  for(const stream of [driver.stdout,driver.stderr]){
    const lines=createInterface({input:stream});driverLines.push(lines);
    lines.on('line',line=>{const code=driverMilestone(line);if(code)driverMilestones.add(code);});
  }
  phase='webdriver_ready';
  await waitFor(async()=>{assert.equal(driver.exitCode,null);try{return (await webdriver(base,'GET','/status')).ready;}catch{return false;}},15000,'WEBDRIVER_READY_TIMEOUT');
  const capabilities={browserName:browser};
  if(browser==='chrome')capabilities['goog:chromeOptions']={args:['--headless=new','--no-first-run','--no-default-browser-check']};
  if(browser==='firefox')capabilities['moz:firefoxOptions']={args:['-headless'],prefs:{'media.peerconnection.ice.loopback':true,'media.peerconnection.ice.default_address_only':false,'media.peerconnection.ice.obfuscate_host_addresses':false,'media.peerconnection.ice.force_interface':'lab0'}};
  phase='webdriver_session';
  const created=await webdriver(base,'POST','/session',{capabilities:{alwaysMatch:capabilities}});session=created.sessionId;
  assert.equal(created.capabilities.browserName.toLowerCase(),browser);
  browserVersion=created.capabilities.browserVersion;
  if(netns && browser==='firefox'){
    const pid=created.capabilities['moz:processID'];
    assert(Number.isInteger(pid) && pid>1,'Firefox must expose its actual process');
    assert.equal((await stat('/proc/'+pid+'/ns/net')).ino,(await stat('/var/run/netns/r4-lab')).ino,'Firefox must stay in the lab namespace');
  }
  phase='webdriver_timeouts';
  await webdriver(base,'POST','/session/'+session+'/timeouts',{script:60000,pageLoad:15000,implicit:0});
  phase='rust_start';
  lab=await startLab({servers,netns});
  const navigate=async()=>{
    phase='page_navigation';
    await webdriver(base,'POST','/session/'+session+'/url',{url:lab.url});
    phase='page_module';
    await waitFor(async()=>await webdriver(base,'POST','/session/'+session+'/execute/sync',{script:'return typeof window.runR4Case === "function";',args:[]}),5000,'PAGE_MODULE_TIMEOUT');
  };
  await navigate();
  const expectedHash=createHash('sha256').update(Uint8Array.from({length:256*1024},(_,i)=>i%251)).digest('hex');
  const turnSnapshot=async()=>turnStats(await readFile(path.join(directory,'turn.log'),'utf8'));
  async function requireReleased(label,attempt){
    try {
      await waitFor(async()=>(await turnSnapshot()).observedActiveAllocations===0,5000,'TURN_ALLOCATION_NOT_RELEASED');
    } catch {
      const failure={case:label,attempt,reason:'TURN_ALLOCATION_NOT_RELEASED',turn:await turnSnapshot()};
      fixtureFailures.push(failure);
      console.error(JSON.stringify({event:'R4_FIXTURE_FAILURE',browser,...failure}));
      throw Error('TURN_ALLOCATION_NOT_RELEASED');
    }
    return turnSnapshot();
  }
  async function runCase(label,mode,denied=false,rustMode=mode,attempt=1,peer='rust'){
    phase=label;
    const turnBefore=await requireReleased(label,attempt),kernelUdpErrorsBefore=udpErrors();
    const result=await webdriver(base,'POST','/session/'+session+'/execute/async',{script:'const done=arguments[arguments.length-1];window.runR4Case(arguments[0]).then(done,()=>done({ok:false,phase:"script",reason:"SCRIPT_FAILED"}));',args:[{mode,rustMode,peer,servers:mode==='relay'?servers:[],blocked:denied}]});
    const turnAfter=await turnSnapshot();
    result.successfulAllocations=turnAfter.allocations-turnBefore.allocations;
    if(!denied && result.ok && result.successfulAllocations<Number(mode==='relay')+Number(rustMode==='relay')){
      result.ok=false;result.phase='allocation_proof';result.reason='TURN_ALLOCATIONS_NOT_PROVEN';
    }
    if(turnAfter.quotaErrors>turnBefore.quotaErrors)result.fixtureReason='TURN_QUOTA_EXCEEDED';
    if(result.ok && !denied){assert.equal(result.bytes,262144);assert.equal(result.packets,17);assert.equal(result.sha256,expectedHash);assert(result.unicodeEcho);}
    const measurement={case:label,attempt,mode,...(peer==='rust'?{rustMode,rust:lab.diagnostics()}:{peer}),...result,turnBefore,turnAfter,kernelUdpErrorsBefore,kernelUdpErrors:udpErrors()};
    results.push(measurement);
    console.log(JSON.stringify({event:result.ok?'R4_CASE':'R4_CASE_FAILURE',browser,version:browserVersion,...measurement}));
    // Close Rust now, not when the next offer arrives; never hide retained allocations by raising quotas.
    await lab.reset();
    measurement.turnReleased=await requireReleased(label,attempt);
    console.log(JSON.stringify({event:'R4_TURN_RELEASED',browser,case:label,attempt,turn:measurement.turnReleased}));
    if(result.fixtureReason)throw Error(result.fixtureReason);
  }
  if(netns)results.push({case:'network_baseline',...await networkProbe(false)});
  for(let attempt=1;attempt<=repetitions;attempt++)await runCase('K_browser_pair_relay','relay',false,'relay',attempt,'browser');
  for(let attempt=1;attempt<=repetitions;attempt++)await runCase('A_direct','direct',false,'direct',attempt);
  for(let attempt=1;attempt<=repetitions;attempt++){
    await runCase('B_udp_relay','relay',false,'relay',attempt);
    await runCase('B1_browser_relay','relay',false,'direct',attempt);
    await runCase('B2_rust_relay','direct',false,'relay',attempt);
  }
  if(netns){
    await lab.stop();lab=undefined;
    phase='block_rule';
    const rule=`table inet r4_udp_block {
  chain incoming {
    type filter hook input priority 0; policy accept;
    meta l4proto udp counter drop
  }
  chain outgoing {
    type filter hook output priority 0; policy accept;
    meta l4proto udp counter drop
  }
}
`;
    try {
      execFileSync('sudo',[...nsPrefix,'nft','--check','-f','-'],{input:rule,stdio:['pipe','pipe','pipe']});
      execFileSync('sudo',[...nsPrefix,'nft','-f','-'],{input:rule,stdio:['pipe','pipe','pipe']});
      blocked=true;
    } catch(error){
      console.error(JSON.stringify({event:'NAMESPACE_RULE_FAILED',exit:error.status,reason:error.stderr?.toString().replace(/\s+/g,' ').slice(0,400)}));
      throw error;
    }
    phase='blocked_network_probe';
    const probe=await networkProbe(true);
    lab=await startLab({servers,netns});await navigate();
    await runCase('C_computer_udp_blocked','relay',true);
    const rules=JSON.parse(nft('-j','list','table','inet','r4_udp_block')).nftables.filter(value=>value.rule).map(value=>value.rule);
    const counters=Object.fromEntries(rules.map(rule=>[rule.chain,rule.expr.find(value=>value.counter).counter.packets]));
    assert(counters.incoming>0 && counters.outgoing>0);results.push({case:'blocked_network_proof',...probe,dropCounters:counters});
    await lab.stop();lab=undefined;
    nft('delete','table','inet','r4_udp_block');blocked=false;
    results.push({case:'network_restored',...await networkProbe(false)});
    lab=await startLab({servers,netns});await navigate();
    for(let attempt=1;attempt<=repetitions;attempt++)await runCase('D_restored_relay','relay',false,'relay',attempt);
  }
  for(const [label,count] of Object.entries(required))assert.equal(results.filter(value=>value.case===label && value.ok===true).length,count,'every required attempt must pass: '+label);
  console.log(JSON.stringify({event:'R4_OK',browser,cases:results.length,natCovered:false}));
} catch(error){console.error(JSON.stringify({event:'R4_FAILED',browser,phase,driverMilestones:[...driverMilestones],code:['TURN_ALLOCATION_NOT_RELEASED','TURN_QUOTA_EXCEEDED'].includes(error.message)?error.message:error.code || error.name,message:error.name==='AssertionError'?error.message.split('\n')[0]:undefined}));process.exitCode=1;}
finally {
  const summary=Object.fromEntries(Object.entries(required).map(([label,expected])=>{
    const cases=results.filter(value=>value.case===label);
    return [label,{passed:cases.filter(value=>value.ok===true).length,attempted:cases.length,required:expected}];
  }));
  console.log(JSON.stringify({event:'R4_SAMPLE_COUNTS',browser,summary}));
  const evidence={commit:process.env.GITHUB_SHA,status:process.exitCode?'failed':'passed',lastPhase:phase,driverMilestones:[...driverMilestones],browser,version:browserVersion,coturn:turnVersion,webrtc:'0.21.1',sendQueueHighWaterBytes:64*1024,topology:netns?'isolated veth namespaces; lab internal default via non-forwarding Rust peer':'private loopback exception',internalDefaultRoute:netns,natCovered:false,repetitions,summary,fixtureFailures,results};
  try{await writeFile(path.join(process.env.RUNNER_TEMP,'r4-'+browser+'.json'),JSON.stringify(evidence,null,2));}catch{console.error('R4_EVIDENCE_WRITE_FAILED');process.exitCode=1;}
  if(lab)await lab.stop();
  if(session)try{await webdriver(base,'DELETE','/session/'+session);}catch{}
  await stopChild(driver);for(const lines of driverLines)lines.close();await stopChild(turn);
  if(blocked)try{nft('delete','table','inet','r4_udp_block');}catch{}
  await rm(directory,{recursive:true,force:true});
}
