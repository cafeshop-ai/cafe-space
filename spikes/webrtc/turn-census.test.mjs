import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import {turnCensus} from './turn-census.mjs';
const password='a'.repeat(64),username='b'.repeat(32);

async function withCli(reply,check) {
  const sockets=new Set(),commands=[];
  const server=net.createServer(socket=>{
    sockets.add(socket);socket.on('close',()=>sockets.delete(socket));socket.on('error',()=>{});
    socket.write(Buffer.from([255,251,1]));
    socket.write('TURN Server\r\nType ? for help\r\nEnter pass');
    setImmediate(()=>socket.write('word: \r\n'));
    let pending='',authenticated=false;
    socket.on('data',chunk=>{
      pending+=chunk.toString();
      while(pending.includes('\n')) {
        const at=pending.indexOf('\n'),line=pending.slice(0,at);pending=pending.slice(at+1);
        if(!authenticated){assert.equal(line,password);authenticated=true;socket.write('> ');}
        else {commands.push(line);assert.equal(line,'ps '+username);reply(socket);}
      }
    });
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try {await check(server.address().port,commands);}
  finally {for(const socket of sockets)socket.destroy();await new Promise(resolve=>server.close(resolve));}
}

test('authenticated read-only CLI census counts live/zero sessions and discards private replies',async()=>{
  for(const sessions of [0,2])await withCli(socket=>{
    const rows=Array.from({length:sessions},(_,i)=>`    ${i+1}) id=00000000000000000${i}, user <${username}>:\r\n      client addr PRIVATE_ADDRESS, server addr PRIVATE_SERVER\r\n`).join('');
    socket.write(rows+'  Total sess');
    setImmediate(()=>socket.write('ions: '+sessions+'\r\n\r\n> '));
  },async(port,commands)=>{
    const result=await turnCensus(port,password,username);
    assert.deepEqual(result,{available:true,sessions});assert.equal(commands.length,1);
    assert(!JSON.stringify(result).includes('PRIVATE_'));assert(!JSON.stringify(result).includes(username));assert(!JSON.stringify(result).includes(password));
  });
});

test('missing, ambiguous, truncated or foreign session replies stay unknown, never zero',async()=>{
  for(const reply of [
    'PRIVATE_ERROR\r\n> ',
    'Total sessions: 0\r\nTotal sessions: 0\r\n> ',
    'Total sessions: 1\r\n> ',
    '1) id=123, user <PRIVATE_OTHER>:\r\nTotal sessions: 1\r\n> ',
    'Total sessions: 9007199254740992\r\n> ',
    'PRIVATE_'.repeat(10000),
  ])await withCli(socket=>socket.write(reply),async port=>{
    const result=await turnCensus(port,password,username);
    assert.equal(result.available,false);assert(!Object.hasOwn(result,'sessions'));
    assert(['CLI_INVALID_RESPONSE','CLI_RESPONSE_LIMIT'].includes(result.reason));assert(!JSON.stringify(result).includes('PRIVATE_'));
  });
  await withCli(socket=>socket.end(),async port=>assert.equal((await turnCensus(port,password,username)).available,false));
  await withCli(()=>{},async port=>assert.deepEqual(await turnCensus(port,password,username),{available:false,reason:'CLI_TIMEOUT'}));
  await assert.rejects(turnCensus(12345,password+'\nshutdown',username),/INVALID_CENSUS_CONFIG/);
  await assert.rejects(turnCensus(12345,password,username+'\ncs 1'),/INVALID_CENSUS_CONFIG/);
  await assert.rejects(turnCensus(0,password,username),/INVALID_CENSUS_CONFIG/);
});

test('unknown greeting receives no credential; denied authentication never sends a command',async()=>{
  for(const recognized of [false,true]) {
    const sockets=new Set(),received=[];
    const server=net.createServer(socket=>{
      sockets.add(socket);socket.on('close',()=>sockets.delete(socket));socket.on('error',()=>{});
      socket.write((recognized?'TURN Server':'PRIVATE_OTHER_SERVICE')+'\r\nEnter password: \r\n');
      socket.on('data',chunk=>{received.push(chunk.toString());socket.write('Enter password: \r\n');});
    });
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const port=server.address().port;
    try {
      const result=await turnCensus(port,password,username);
      assert.deepEqual(result,{available:false,reason:recognized?'CLI_AUTH_FAILED':'CLI_TIMEOUT'});
      assert.deepEqual(received,recognized?[password+'\n']:[]);
    } finally {for(const socket of sockets)socket.destroy();await new Promise(resolve=>server.close(resolve));}
    assert.deepEqual(await turnCensus(port,password,username),{available:false,reason:'CLI_CONNECTION_FAILED'});
  }
});
