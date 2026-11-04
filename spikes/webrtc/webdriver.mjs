// Emit only fixed codes; WebDriver messages and browser command lines can contain private paths.
export function driverMilestone(line) {
  if (/Marionette.*Listening on port/.test(line)) return 'MARIONETTE_LISTENING';
  if (/Connection to Marionette established/.test(line)) return 'MARIONETTE_CONNECTED';
  if (/Running command:/.test(line)) return 'BROWSER_LAUNCH_REQUESTED';
  if (/Process unexpectedly closed/.test(line)) return 'BROWSER_EXITED';
  if (/Listening on/.test(line)) return 'DRIVER_LISTENING';
}
const failure = code => Object.assign(new Error(code), {code});
export async function webdriver(base,method,route,body) {
  let response;
  try {
    response=await fetch(base+route,{method,headers:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(70000)});
  } catch(error) {
    throw failure(error?.name==='TimeoutError'?'WEBDRIVER_REQUEST_TIMEOUT':'WEBDRIVER_TRANSPORT_FAILED');
  }
  let result;
  try { result=await response.json(); } catch { throw failure('WEBDRIVER_INVALID_RESPONSE'); }
  if(!result || typeof result!=='object' || !Object.hasOwn(result,'value')) throw failure('WEBDRIVER_INVALID_RESPONSE');
  if(!response.ok || result.value?.error) {
    const known=['session not created','invalid session id','timeout','script timeout','no such window'];
    const code=known.includes(result?.value?.error)?result.value.error.toUpperCase().replaceAll(' ','_'):'COMMAND_FAILED';
    throw failure('WEBDRIVER_'+code);
  }
  return result?.value;
}
