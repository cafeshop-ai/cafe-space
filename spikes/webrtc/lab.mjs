import http from 'node:http';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
const here = path.dirname(fileURLToPath(import.meta.url));
const binary = path.join(here,'target/debug/cafe-space-webrtc-spike'+(process.platform==='win32'?'.exe':''));
const modules = new Map(await Promise.all(['pcd1.mjs','browser.mjs'].map(async name=>['/'+name,await readFile(path.join(here,name))])));
export async function startLab({servers=[],netns=false,binaryPath=binary}={}) {
  if (netns && (process.platform!=='linux' || !process.getuid() || process.env.RUNNER_ENVIRONMENT!=='github-hosted')) throw Error('INVALID_LAB_NAMESPACE');
  const args=['-n','ip','netns','exec','r4-rust','sudo','-n','-u','#'+process.getuid?.(),'--','env','CAFE_SPACE_R4_NETNS=1','RUNNER_ENVIRONMENT=github-hosted',binaryPath];
  const child = netns ? spawn('sudo',args,{stdio:['pipe','pipe','ignore']}) : spawn(binaryPath,[],{stdio:['pipe','pipe','ignore']});
  let waiting, fatal;
  const diagnostics=[];
  let readyResolve,readyReject;
  const ready = new Promise((resolve,reject)=>{readyResolve=resolve;readyReject=reject;});
  const fail = () => {fatal = Error('RUST_PROBE_EXITED');readyReject(fatal);waiting?.reject(fatal);waiting=undefined;};
  child.on('error',fail); child.on('exit',fail); child.stdin.on('error',fail);
  const lines=createInterface({input:child.stdout});
  lines.on('line',line=>{
    let value; try {value=JSON.parse(line);} catch {fail();return;}
    if(value.event==='ready') readyResolve();
    else if(value.event==='diagnostic'){
      if(/^[A-Z_]{1,32}$/.test(value.code) && Number.isSafeInteger(value.bytes) && value.bytes>=0 && value.bytes<=1024*1024){diagnostics.push({code:value.code,bytes:value.bytes});if(diagnostics.length>64)diagnostics.shift();}
    }
    else if(waiting) {const pending=waiting;waiting=undefined;pending.resolve(value);}
  });
  const readyTimer=setTimeout(()=>readyReject(Error('RUST_READY_TIMEOUT')),15000);
  try {await ready;} catch(error) {child.kill();throw error;} finally {clearTimeout(readyTimer);}
  async function signal(value) {
    if(fatal) throw fatal;
    if(waiting) throw Error('LAB_BUSY');
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{waiting=undefined;reject(Error('RUST_SIGNAL_TIMEOUT'));},20000);
      waiting={resolve:value=>{clearTimeout(timer);resolve(value);},reject:error=>{clearTimeout(timer);reject(error);}};
      child.stdin.write(JSON.stringify(value)+'\n');
    });
  }
  const server=http.createServer(async(req,res)=>{
    try {
      const host='127.0.0.1:'+server.address().port;
      if(req.headers.host!==host || (req.headers.origin && req.headers.origin!=='http://'+host)) {res.writeHead(403).end();return;}
      res.setHeader('Cache-Control','no-store');
      res.setHeader('X-Content-Type-Options','nosniff');
      if(req.method==='GET' && req.url==='/') {
        res.setHeader('Content-Type','text/html; charset=utf-8');
        res.end('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Isolated R4 probe</title><main><h1>Isolated R4 probe</h1><p>Anonymous loopback experiment. No saved device identity.</p><output id="status" aria-live="polite">Ready for automated checks.</output></main><script type="module" src="/browser.mjs"></script></html>');return;
      }
      if(req.method==='GET' && modules.has(req.url)) {res.setHeader('Content-Type','text/javascript; charset=utf-8');res.end(modules.get(req.url));return;}
      if(req.method!=='POST' || req.url!=='/offer') {res.writeHead(404).end();return;}
      if(req.headers['content-type']?.split(';')[0]!=='application/json') {res.writeHead(415).end();return;}
      const chunks=[];let bytes=0;
      for await(const chunk of req) {bytes+=chunk.length;if(bytes>60000){res.writeHead(413).end();return;}chunks.push(chunk);}
      const body=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(Buffer.concat(chunks)));
      if(!['direct','relay'].includes(body.mode) || !body.offer || (body.mode==='relay' && servers.length!==1)) {res.writeHead(400).end();return;}
      if(waiting) {res.writeHead(409).end();return;}
      diagnostics.length=0;
      const answer=await signal({mode:body.mode,servers:body.mode==='relay'?servers:[],offer:body.offer});
      if(answer.event!=='answer') throw Error('INVALID_ANSWER');
      res.setHeader('Content-Type','application/json');res.end(JSON.stringify(answer));
    } catch {if(!res.headersSent)res.writeHead(400);res.end();}
  });
  server.requestTimeout=10000;server.headersTimeout=10000;
  await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
  return {url:'http://127.0.0.1:'+server.address().port,diagnostics:()=>diagnostics.slice(),async stop(){
    server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
    child.stdin.end();
    if(child.exitCode===null) await new Promise(resolve=>{const timer=setTimeout(()=>{child.kill();resolve();},5000);child.once('exit',()=>{clearTimeout(timer);resolve();});});
    lines.close();
  }};
}
if(process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const lab=await startLab();console.log(JSON.stringify({url:lab.url,isolated:true}));
  process.once('SIGINT',async()=>{await lab.stop();process.exit();});
  process.once('SIGTERM',async()=>{await lab.stop();process.exit();});
}
