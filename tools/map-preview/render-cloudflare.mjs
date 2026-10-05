import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import puppeteer from 'puppeteer-core';

const hash=b=>createHash('sha256').update(b).digest('hex');
const root=path.dirname(fileURLToPath(import.meta.url));
export function apiFailure(status,body,retryAfter){
 const daily=status===429&&/daily|today|time limit|browser time|quota/i.test(body);
 const seconds=Number(retryAfter)||Math.ceil((Date.parse(retryAfter)-Date.now())/1000)||60;
 return {category:status===401||status===403?'auth':daily?'quota':status===429?'rate_limit':'transport',retry_after_seconds:daily?Math.ceil((86400000-Date.now()%86400000)/1000):Math.min(86400,Math.max(20,seconds))};
}
async function asset(...choices){for(const p of choices){try{return await fs.readFile(p);}catch(e){if(e.code!=='ENOENT')throw e;}}throw Error('Required renderer asset is missing');}
async function atomic(file,data){const tmp=file+'.tmp';await fs.writeFile(tmp,data,{mode:0o600});await fs.rename(tmp,file);}

export async function closeOwnedSession(browser,sessionId,endpoint,sessionFile,api,closeTimeout=3000){
 if(browser){
  let closeTimer;
  try{await Promise.race([browser.close(),new Promise((_,reject)=>{closeTimer=setTimeout(()=>reject(Error('CDP close deadline')),closeTimeout);})]);}
  catch{browser.disconnect();}finally{clearTimeout(closeTimer);}
 }
 if(sessionId){
  await api(`${endpoint}/${sessionId}`,'DELETE',true);
  if(sessionFile)await fs.unlink(sessionFile).catch(e=>{if(e.code!=='ENOENT')throw e;});
 }
}

export async function main(input,output){
 await fs.mkdir(output,{recursive:true,mode:0o700});
 const raw=await fs.readFile(input);if(raw.length>4_000_000)throw Error('Map snapshot exceeds limit');
 const snapshot=JSON.parse(raw),targets=snapshot._render?.targets??['n5','wc'];
 if(!Number.isInteger(snapshot.ward)||snapshot.ward<1||snapshot.ward>50||!snapshot.boundary||!Array.isArray(targets)||!targets.length||targets.some(x=>!['n5','wc','ward-base'].includes(x)))throw Error('Invalid map input');
 const account=process.env.ADU_CF_ACCOUNT_ID,tokenFile=process.env.ADU_CF_TOKEN_FILE;
 if(!/^[a-f0-9]{32}$/i.test(account??'')||!tokenFile)throw Error('Cloudflare account and token file are required');
 const token=(await fs.readFile(tokenFile,'utf8')).trim();if(!token||token.length>4096||/\s/.test(token))throw Error('Invalid Cloudflare token file');
 const endpoint=`https://api.cloudflare.com/client/v4/accounts/${account}/browser-run/devtools/browser`;
 const sessionFile=process.env.ADU_CF_SESSION_FILE;
 const timeout=Math.min(120,Math.max(10,Number(process.env.ADU_MAP_TIMEOUT_SECONDS)||90))*1000;
 let browser,sessionId,timer,aborted=false,phase='setup';
 const abort=new AbortController(),errors=[],consoleErrors=[];
 async function api(url,method,cleanup=false){
  const response=await fetch(url,{method,headers:{Authorization:`Bearer ${token}`},signal:cleanup?AbortSignal.timeout(8000):abort.signal,redirect:'error'});
  if(method==='DELETE'&&response.status===404)return {};
  const reader=response.body.getReader(),chunks=[];let size=0;
  for(;;){const {done,value}=await reader.read();if(done)break;size+=value.length;if(size>32_000){await reader.cancel();throw Error('Unexpected Cloudflare response');}chunks.push(Buffer.from(value));}
  const body=Buffer.concat(chunks).toString('utf8');
  if(!response.ok){const e=Error('Cloudflare session request failed');e.renderFailure=apiFailure(response.status,body,response.headers.get('retry-after'));throw e;}
  return JSON.parse(body);
 }
 let interrupt;
 const interrupted=new Promise((_,reject)=>{interrupt=()=>{aborted=true;abort.abort();const e=Error('Map rendering interrupted');e.renderFailure={category:'timeout'};reject(e);};timer=setTimeout(interrupt,timeout);});
 process.once('SIGTERM',interrupt);process.once('SIGINT',interrupt);
 try{
  await Promise.race([interrupted,(async()=>{
   if(sessionFile){
    const previous=await fs.readFile(sessionFile,'utf8').then(JSON.parse).catch(e=>{if(e.code==='ENOENT')return null;throw e;});
    if(previous){
     if(previous.account!==account||!/^[-a-f0-9]{36}$/.test(previous.session_id))throw Error('Invalid abandoned session record');
     await api(`${endpoint}/${previous.session_id}`,'DELETE');await fs.unlink(sessionFile);
    }
   }
   const [vendor,script,baseBytes,big,bodyFont]=await Promise.all([
    asset(path.join(root,'vendor/maplibre-gl.js'),path.join(root,'node_modules/maplibre-gl/dist/maplibre-gl.js')),
    asset(path.join(root,'render-live.js')),asset(path.join(root,'data/base-style.json')),
    asset(path.join(root,'fonts/BigShouldersText-Bold.ttf'),path.resolve(root,'../../assets/fonts/BigShouldersText-Bold.ttf')),
    asset(path.join(root,'fonts/Roboto.ttf'),path.resolve(root,'../../assets/fonts/Roboto.ttf'))]);
   const base=JSON.parse(baseBytes);
   phase='acquire';const acquired=await api(endpoint+'?keep_alive=60000','POST');
   if(!/^[-a-f0-9]{36}$/.test(acquired.sessionId??''))throw Error('Cloudflare did not return a session ID');
   sessionId=acquired.sessionId;
   if(sessionFile)await atomic(sessionFile,JSON.stringify({account,session_id:sessionId}));
   if(aborted)throw Error('Rendering was interrupted');
   phase='connect';browser=await puppeteer.connect({browserWSEndpoint:`wss://api.cloudflare.com/client/v4/accounts/${account}/browser-run/devtools/browser/${sessionId}`,headers:{Authorization:`Bearer ${token}`},protocolTimeout:timeout});
   if(aborted)throw Error('Rendering was interrupted');
   phase='page';const page=await browser.newPage();
   // Module workers need a concrete document origin. Fulfill this reserved URL
   // locally in CDP; it never contacts a host or requires a public render page.
   await page.setRequestInterception(true);
   const origin='https://adu-map-render.invalid/';
   const initial=request=>(request.url()===origin?request.respond({status:200,contentType:'text/html',body:'<!doctype html><html><body></body></html>'}):request.continue()).catch(()=>{});
   page.on('request',initial);
   await page.goto(origin,{waitUntil:'domcontentloaded',timeout});
   page.off('request',initial);await page.setRequestInterception(false);
   await page.setViewport({width:1080,height:1080,deviceScaleFactor:2});
   let pageReject;const pageFailure=new Promise((_,reject)=>{pageReject=reject;});
   pageFailure.catch(()=>{}); // Observe errors even before script injection finishes.
   page.on('pageerror',e=>{errors.push(String(e.message).slice(0,1000));pageReject(Error('Map browser script failed'));});
   page.on('console',e=>{if(e.type()==='error')consoleErrors.push(e.text().slice(0,1000));});
   const images=new Map();let manifest;
   await page.exposeFunction('aduSaveBlob',async(name,b64)=>{
    const allowed=targets.map(x=>x==='ward-base'?'ward-base.png':x+'.jpg');
    if(!allowed.includes(name)||typeof b64!=='string'||b64.length>11_000_000)throw Error('Invalid image export');
    const bytes=Buffer.from(b64,'base64'),limit=name.endsWith('.png')?8_000_000:2_000_000;
    if(bytes.length<1000||bytes.length>limit)throw Error('Map image exceeds limit');
    await atomic(path.join(output,name),bytes);images.set(name,{sha256:hash(bytes),bytes:bytes.length});
   });
   await page.exposeFunction('aduSaveVerification',value=>{manifest=value;});
   await page.setContent(`<style>@font-face{font-family:Big;src:url(data:font/ttf;base64,${big.toString('base64')})}@font-face{font-family:Roboto;src:url(data:font/ttf;base64,${bodyFont.toString('base64')})}html,body{margin:0;background:#000;color:white;font-family:Roboto}#map{width:1080px;height:848px}#status{position:absolute;top:0;left:0}</style><div id="status">Loading map</div><div id="map"></div>`);
   await page.evaluate(input=>{
    window.aduRenderInput=input;
    window.aduExportBlob=async(name,blob)=>{const b64=await new Promise(resolve=>{const r=new FileReader();r.onload=()=>resolve(r.result.split(',')[1]);r.readAsDataURL(blob);});await window.aduSaveBlob(name,b64);};
    window.aduExportVerification=async value=>{await window.aduSaveVerification(value);};
   },{snapshot,base,targets});
   phase='render';await page.addScriptTag({content:vendor.toString()});await page.addScriptTag({type:'module',content:script.toString()});
   await Promise.race([pageFailure,page.waitForFunction(()=>document.querySelector('#status').textContent==='Scorecard maps complete',{timeout})]);
   if(errors.length||!manifest||manifest.renders.length!==targets.length)throw Error('Incomplete map render');
   for(const r of manifest.renders){const name=r.id==='ward-base'?'ward-base.png':r.id+'.jpg',image=images.get(name);if(!image||image.bytes!==r.bytes||r.errors.length)throw Error('Invalid map verification');Object.assign(r,image,{name});}
   Object.assign(manifest,{schema_version:1,input_sha256:hash(raw),asset_sha256:process.env.ADU_MAP_ASSET_DIGEST});
   await atomic(path.join(output,'verification.json'),JSON.stringify(manifest));
  })()]);
 }catch(error){
  await atomic(path.join(output,'render-error.json'),JSON.stringify({...error.renderFailure??{category:aborted?'timeout':'transport'},phase,page_errors:errors.slice(0,10),console_errors:consoleErrors.slice(0,10)}));
  throw Error('Cloudflare map rendering failed; see structured render error');
 }finally{
  clearTimeout(timer);process.removeListener('SIGTERM',interrupt);process.removeListener('SIGINT',interrupt);
  // Explicit HTTP closure also covers a disconnected or partially acquired CDP session.
  try{
   await closeOwnedSession(browser,sessionId,endpoint,sessionFile,api);
   if(sessionId){
    const errorFile=path.join(output,'render-error.json');
    const failure=await fs.readFile(errorFile,'utf8').then(JSON.parse).catch(e=>{if(e.code==='ENOENT')return null;throw e;});
    if(failure)await atomic(errorFile,JSON.stringify({...failure,session_closed:true}));
   }
  }catch{
   await atomic(path.join(output,'render-error.json'),JSON.stringify({category:'cleanup'}));
   throw Error('Cloudflare session cleanup failed');
  }
 }
}
