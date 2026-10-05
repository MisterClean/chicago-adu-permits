import fs from 'node:fs/promises';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import puppeteer from 'puppeteer-core';
import {fileURLToPath} from 'node:url';
// Compare copies of the renderer; this tool never changes application code.
const args=process.argv.slice(2);
if(args.includes('--help')){
 console.log('Usage: CF_ACCOUNT_ID=... node tools/cloudflare-benchmark/benchmark.mjs --snapshot FILE --out DIR [--variants baseline,lean,lean,baseline]');
 process.exit(0);
}
function option(name,fallback){const i=args.indexOf(name);return i<0?fallback:args[i+1];}
const account=process.env.CF_ACCOUNT_ID;
const input=option('--snapshot'),output=option('--out');
if(!account||!input||!output)throw Error('CF_ACCOUNT_ID, --snapshot and --out are required; see --help');
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const dir=path.resolve(output),assets=path.join(root,'tools/map-preview');
await fs.mkdir(dir,{recursive:true});
const [snapshot,base]=await Promise.all([fs.readFile(input,'utf8').then(JSON.parse),fs.readFile(path.join(assets,'data/base-style.json'),'utf8').then(JSON.parse)]);
if(!Number.isInteger(snapshot.ward)||snapshot.ward<1||snapshot.ward>50||!snapshot.boundary||!snapshot.points?.some(p=>p.id===snapshot.focus?.id))throw Error('Invalid map snapshot');
const [big,roboto,vendor,original]=await Promise.all([
 fs.readFile(path.join(root,'assets/fonts/BigShouldersText-Bold.ttf')).then(b=>b.toString('base64')),
 fs.readFile(path.join(root,'assets/fonts/Roboto.ttf')).then(b=>b.toString('base64')),
 fs.readFile(path.join(assets,'node_modules/maplibre-gl/dist/maplibre-gl.js'),'utf8'),
 fs.readFile(path.join(root,'tools/cloudflare-benchmark/baseline-render.js'),'utf8')]);
const token=process.env.CLOUDFLARE_API_TOKEN||JSON.parse(execFileSync('wrangler',['auth','token','--json'],{encoding:'utf8',stdio:['ignore','pipe','pipe']})).token;
if(!token)throw Error('Cloudflare token unavailable');
function replace(s,from,to){if(!s.includes(from))throw Error('Renderer patch no longer matches: '+from.slice(0,60));return s.replace(from,to);}
function rendererFor(variant){
 let s=replace(original,"const [snapshot,base]=await Promise.all([fetch('/payload.json').then(r=>r.json()),fetch('/data/base-style.json').then(r=>r.json())]);",'const {snapshot,base}=window.input;');
 s=replace(s,"const map=new maplibregl.Map(","await window.markPhase(id);const phaseTimes={start:performance.now()};const map=new maplibregl.Map(");
 s=replace(s,"if(ward){\n  map.fitBounds", "phaseTimes.loaded=performance.now();\n if(ward){\n  map.fitBounds");
 s=replace(s,"await waitIdle(map);", "await waitIdle(map);phaseTimes.idle=performance.now();");
 s=replace(s,"const response=await fetch(`/export/${id}.jpg`,{method:'POST',body:blob});if(!response.ok)throw Error('Image export failed');",`phaseTimes.jpeg=performance.now();await window.saveExport(id+'.jpg',await new Promise(resolve=>{const reader=new FileReader();reader.onload=()=>resolve(reader.result.split(',')[1]);reader.readAsDataURL(blob);}));`);
 s=replace(s,"results.push({id,bytes:blob.size,width:2160,height:2160,errors});","results.push({id,bytes:blob.size,width:2160,height:2160,errors,phaseTimes,camera:{center:map.getCenter().toArray(),zoom:map.getZoom(),bearing:map.getBearing(),pitch:map.getPitch()},pointPixels:points.map(f=>({id:f.id,p:map.project([f.location.longitude,f.location.latitude])}))});");
 s=replace(s,"const response=await fetch('/export/verification.json',{method:'POST',body:JSON.stringify({mode:snapshot.mode??'scorecard',source_run:snapshot.source_run,ward:snapshot.ward,focus:snapshot.focus.id,renders:results})});\nif(!response.ok)throw Error('Verification export failed');","await window.saveVerification({mode:snapshot.mode??'scorecard',source_run:snapshot.source_run,ward:snapshot.ward,focus:snapshot.focus.id,renders:results});");
 if(variant==='final-camera'||variant==='lean'){
  s=replace(s,"style:style(t,ward),center,zoom:16.58,", "style:style(t,ward),center,zoom:16.58,...(ward?{bounds:[[bounds[0],bounds[1]],[bounds[2],bounds[3]]],fitBoundsOptions:{padding:{top:44,bottom:54,left:76,right:76}}}:{}),");
  s=replace(s,"map.fitBounds([[bounds[0],bounds[1]],[bounds[2],bounds[3]]],{padding:{top:44,bottom:54,left:76,right:76},duration:0});", "// Final ward camera was set before any tiles were requested.");
 }
 if(variant==='lean'){
  s=replace(s,"map.once('load',", "map.once('style.load',");
  s=replace(s,"const s=structuredClone(base);", "const s=structuredClone(base);delete s.sprite;delete s.sources.ne2_shaded;");
 }
 return s;
}
const variants=option('--variants','baseline,lean,lean,baseline').split(',');
const reports=[];let previousStart=-Infinity;
for(let i=0;i<variants.length;i++){
 const variant=variants[i];if(!['baseline','final-camera','lean'].includes(variant))throw Error('Unknown variant');
 const name=`${Date.now()}-${variant}`,out=path.join(dir,name);await fs.mkdir(out,{recursive:true});
 const cooldown=20000-(performance.now()-previousStart);if(cooldown>0)await new Promise(resolve=>setTimeout(resolve,cooldown));
 const start=performance.now();previousStart=start;const report={variant,name,started_at:new Date().toISOString(),requests:[]};
 let browser,phase='setup';const requests=new Map();report.peak_controller_rss_bytes=process.memoryUsage().rss;const memoryTimer=setInterval(()=>{report.peak_controller_rss_bytes=Math.max(report.peak_controller_rss_bytes,process.memoryUsage().rss);},250);
 try{
  browser=await puppeteer.connect({browserWSEndpoint:`wss://api.cloudflare.com/client/v4/accounts/${account}/browser-run/devtools/browser?keep_alive=60000`,headers:{Authorization:`Bearer ${token}`},protocolTimeout:150000});
  report.connected_seconds=(performance.now()-start)/1000;
  const page=await browser.newPage();
  await page.setRequestInterception(true);
  const origin='https://adu-map-render.invalid/';
  const initial=request=>(request.url()===origin?request.respond({status:200,contentType:'text/html',body:'<!doctype html><html><body></body></html>'}):request.continue()).catch(()=>{});
  page.on('request',initial);await page.goto(origin,{waitUntil:'domcontentloaded',timeout:30000});page.off('request',initial);await page.setRequestInterception(false);
  await page.setViewport({width:1080,height:1080,deviceScaleFactor:2});
  const cdp=await page.createCDPSession();await cdp.send('Network.enable');await cdp.send('Network.clearBrowserCache');
  function monitor(session,label){
   const key=id=>label+':'+id;
   session.on('Network.requestWillBeSent',e=>{if(e.request.url.startsWith('https://'))requests.set(key(e.requestId),{url:e.request.url,phase,target:label,started_seconds:(performance.now()-start)/1000});});
   session.on('Network.responseReceived',e=>{const r=requests.get(key(e.requestId));if(r)Object.assign(r,{status:e.response.status,fromDiskCache:e.response.fromDiskCache??false});});
   session.on('Network.loadingFinished',e=>{const r=requests.get(key(e.requestId));if(r)Object.assign(r,{encodedBytes:e.encodedDataLength,ended_seconds:(performance.now()-start)/1000});});
   session.on('Network.loadingFailed',e=>{const r=requests.get(key(e.requestId));if(r)Object.assign(r,{failed:e.errorText,canceled:e.canceled??false});});
  }
  monitor(cdp,'page');
  cdp.on('Target.attachedToTarget',async e=>{
   const session=cdp.connection().session(e.sessionId);
   try{monitor(session,e.targetInfo.targetId);await session.send('Network.enable');await session.send('Runtime.runIfWaitingForDebugger');}catch(error){console.error('Worker instrumentation:',error.message);}
  });
  await cdp.send('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:true,flatten:true,filter:[{type:'worker'},{exclude:true}]});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.exposeFunction('markPhase',id=>{phase=id;});
  await page.exposeFunction('saveExport',async(name,data)=>{await fs.writeFile(path.join(out,name),Buffer.from(data,'base64'));console.log(variant,name,((performance.now()-start)/1000).toFixed(2)+'s');});
  await page.exposeFunction('saveVerification',async(v)=>{report.verification=v;await fs.writeFile(path.join(out,'verification.json'),JSON.stringify(v,null,2));});
  await page.setContent(`<style>@font-face{font-family:Big;src:url(data:font/ttf;base64,${big})}@font-face{font-family:Roboto;src:url(data:font/ttf;base64,${roboto})}html,body{margin:0;background:#000;color:white;font-family:Roboto}#map{width:1080px;height:848px}#status{position:absolute;top:0;left:0}</style><div id="status">Loading map…</div><div id="map"></div>`);
  await page.evaluate(input=>{window.input=input;},{snapshot,base});
  await page.addScriptTag({content:vendor});await page.addScriptTag({type:'module',content:rendererFor(variant)});
  await page.waitForFunction(()=>document.querySelector('#status').textContent==='Scorecard maps complete',{timeout:120000});
  if(errors.length)throw Error(errors.join('; '));
  report.completed_seconds=(performance.now()-start)/1000;
 }catch(e){report.error=e.message;console.error(variant,'failed:',e.message);}
 finally{
  if(browser){try{await browser.close();report.explicit_close=true;}catch(error){report.close_error=error.message;}}clearInterval(memoryTimer);report.closed_seconds=(performance.now()-start)/1000;
  report.requests=[...requests.values()];
  report.network=Object.fromEntries(['n5','wc'].map(p=>{const rs=report.requests.filter(r=>r.phase===p),tiles=rs.filter(r=>/\/planet\/.*\.pbf/.test(r.url));return [p,{requests:rs.length,encodedBytes:rs.reduce((n,r)=>n+(r.encodedBytes??0),0),tileRequests:tiles.length,uniqueTiles:new Set(tiles.map(r=>r.url)).size,tileEncodedBytes:tiles.reduce((n,r)=>n+(r.encodedBytes??0),0),failures:rs.filter(r=>r.failed&&!r.canceled).length,canceled:rs.filter(r=>r.canceled).length}];}));
  await fs.writeFile(path.join(out,'report.json'),JSON.stringify(report,null,2));reports.push(report);
  console.log('REPORT',JSON.stringify({variant,name,closed_seconds:report.closed_seconds,network:report.network,error:report.error}));
 }
 if(report.error)break;
}
await fs.writeFile(path.join(dir,`batch-${Date.now()}.json`),JSON.stringify(reports,null,2));

if(reports.some(r=>r.error||r.close_error))process.exitCode=1;
