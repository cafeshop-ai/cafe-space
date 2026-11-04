import test from 'node:test';
import assert from 'node:assert/strict';
import {driverMilestone,webdriver} from './webdriver.mjs';

test('driver milestones discard command lines, profile paths and arbitrary logs',()=>{
  assert.equal(driverMilestone('mozrunner Running command: PRIVATE_PATH PRIVATE_SECRET'),'BROWSER_LAUNCH_REQUESTED');
  assert.equal(driverMilestone('Marionette INFO Listening on port 12345'),'MARIONETTE_LISTENING');
  assert.equal(driverMilestone('Connection to Marionette established'),'MARIONETTE_CONNECTED');
  assert.equal(driverMilestone('Process unexpectedly closed with status 1 PRIVATE_PATH'),'BROWSER_EXITED');
  assert.equal(driverMilestone('geckodriver INFO Listening on 127.0.0.1:12345'),'DRIVER_LISTENING');
  assert.equal(driverMilestone('PRIVATE_SECRET'),undefined);
});
test('WebDriver keeps request deadlines and returns only static failure codes',async()=>{
  const previous=globalThis.fetch,previousTimeout=AbortSignal.timeout,timeouts=[];
  AbortSignal.timeout=ms=>{timeouts.push(ms);return new AbortController().signal;};
  try {
    let request;
    globalThis.fetch=async(url,options)=>{request={url,...options};return {ok:true,json:async()=>({value:{ready:true}})};};
    assert.deepEqual(await webdriver('http://127.0.0.1:12345','POST','/session',{capabilities:{}}),{ready:true});
    assert.equal(request.url,'http://127.0.0.1:12345/session');
    assert.equal(request.method,'POST');assert.equal(request.body,'{"capabilities":{}}');
    assert.equal(request.signal.aborted,false);
    for(const [upstream,code] of [['session not created','SESSION_NOT_CREATED'],['timeout','TIMEOUT'],['script timeout','SCRIPT_TIMEOUT'],['PRIVATE_SECRET','COMMAND_FAILED']]) {
      globalThis.fetch=async()=>({ok:false,json:async()=>({value:{error:upstream,message:'PRIVATE_SECRET',stacktrace:'PRIVATE_PATH'}})});
      await assert.rejects(webdriver('http://localhost','GET','/status'),error=>{
        assert.equal(error.code,'WEBDRIVER_'+code);assert.equal(error.message,error.code);
        assert(!JSON.stringify(error).includes('PRIVATE_'));return true;
      });
    }
    globalThis.fetch=async()=>{throw Object.assign(Error('PRIVATE_SECRET'),{name:'TimeoutError'});};
    await assert.rejects(webdriver('http://localhost','GET','/status'),{code:'WEBDRIVER_REQUEST_TIMEOUT'});
    globalThis.fetch=async()=>{throw Error('PRIVATE_SECRET');};
    await assert.rejects(webdriver('http://localhost','GET','/status'),{code:'WEBDRIVER_TRANSPORT_FAILED'});
    globalThis.fetch=async()=>({ok:true,json:async()=>{throw Error('PRIVATE_SECRET');}});
    await assert.rejects(webdriver('http://localhost','GET','/status'),{code:'WEBDRIVER_INVALID_RESPONSE'});
    for(const value of [null,{}]) {
      globalThis.fetch=async()=>({ok:true,json:async()=>value});
      await assert.rejects(webdriver('http://localhost','GET','/status'),{code:'WEBDRIVER_INVALID_RESPONSE'});
    }
    assert(timeouts.length>0 && timeouts.every(ms=>ms===70000));
  } finally {globalThis.fetch=previous;AbortSignal.timeout=previousTimeout;}
});
