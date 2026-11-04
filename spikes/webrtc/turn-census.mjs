// Only the owned loopback coturn CLI is queried; raw replies and credentials never leave this module.
import net from 'node:net';

export async function turnCensus(port,password,username) {
  if(!Number.isInteger(port) || port<1024 || port>65535 || !/^[a-f0-9]{64}$/.test(password) || !/^[a-f0-9]{32}$/.test(username))throw Error('INVALID_CENSUS_CONFIG');
  return new Promise(resolve=>{
    let text='',bytes=0,state='greeting',done=false;
    const socket=net.connect({host:'127.0.0.1',port});
    const finish=result=>{if(done)return;done=true;clearTimeout(timer);socket.destroy();resolve(result);};
    const unavailable=reason=>finish({available:false,reason});
    const timer=setTimeout(()=>unavailable('CLI_TIMEOUT'),1000);
    socket.on('error',()=>unavailable('CLI_CONNECTION_FAILED'));
    socket.on('close',()=>unavailable('CLI_CLOSED'));
    socket.on('data',chunk=>{
      bytes+=chunk.length;
      if(bytes>65536)return unavailable('CLI_RESPONSE_LIMIT');
      text+=chunk.toString('utf8');
      if(state==='greeting') {
        if(!text.includes('TURN Server') || !text.includes('Enter password: '))return;
        state='authentication';text='';socket.write(password+'\n');return;
      }
      if(state==='authentication' && text.includes('Enter password: '))return unavailable('CLI_AUTH_FAILED');
      if(!text.endsWith('> '))return;
      if(state==='authentication') {
        state='sessions';text='';socket.write('ps '+username+'\n');return;
      }
      const totals=[...text.matchAll(/^\s*Total sessions: (\d+)\r?$/gm)];
      const sessions=totals.length===1?Number(totals[0][1]):NaN;
      const rows=[...text.matchAll(/^\s*\d+\) id=\d+, user <([^>]+)>:\r?$/gm)];
      if(!Number.isSafeInteger(sessions) || sessions!==rows.length || rows.some(row=>row[1]!==username))return unavailable('CLI_INVALID_RESPONSE');
      finish({available:true,sessions});
    });
  });
}
