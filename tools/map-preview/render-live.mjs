import http from 'node:http';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const root=path.dirname(fileURLToPath(import.meta.url));
const [input,output]=process.argv.slice(2);
if(!input||!output)throw Error('Usage: node render-live.mjs SNAPSHOT.json OUTPUT_DIR');
const payload=await fs.readFile(input);
const snapshot=JSON.parse(payload);
if(!Number.isInteger(snapshot.ward)||snapshot.ward<1||snapshot.ward>50||!snapshot.boundary)throw Error('Invalid scorecard snapshot');
await fs.mkdir(output,{recursive:true,mode:0o700});
const browserPath=process.env.CHROME_BIN||(process.platform==='darwin'?'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome':'/usr/bin/google-chrome');
async function asset(...choices){for(const candidate of choices){if(await fs.stat(candidate).catch(()=>false))return candidate;}throw Error(`Missing renderer asset: ${choices[0]}`);}
const files=new Map([
 ['/render-live.html',path.join(root,'render-live.html')],['/render-live.js',path.join(root,'render-live.js')],
 ['/data/base-style.json',path.join(root,'data/base-style.json')],
 ['/vendor/maplibre-gl.js',await asset(path.join(root,'vendor/maplibre-gl.js'),path.join(root,'node_modules/maplibre-gl/dist/maplibre-gl.js'))],
 ['/fonts/BigShouldersText-Bold.ttf',await asset(path.join(root,'fonts/BigShouldersText-Bold.ttf'),path.resolve(root,'../../assets/fonts/BigShouldersText-Bold.ttf'))],
 ['/fonts/Roboto.ttf',await asset(path.join(root,'fonts/Roboto.ttf'),path.resolve(root,'../../assets/fonts/Roboto.ttf'))],
]);
const types={'.html':'text/html','.js':'text/javascript','.json':'application/json','.ttf':'font/ttf'};
let completed;
const done=new Promise((resolve,reject)=>{completed={resolve,reject};});
const server=http.createServer(async(req,res)=>{
 try{
  const pathname=new URL(req.url,'http://localhost').pathname;
  if(req.method==='GET'){
   if(pathname!=='/payload.json'&&!files.has(pathname)){res.writeHead(404).end();return;}
   const data=pathname==='/payload.json'?payload:await fs.readFile(files.get(pathname));
   res.writeHead(200,{'Content-Type':pathname==='/payload.json'?'application/json':types[path.extname(pathname)]||'application/octet-stream','Cache-Control':'no-store'}).end(data);
   return;
  }
  const match=/^\/export\/(n5\.jpg|wc\.jpg|verification\.json|error\.txt)$/.exec(pathname);
  if(req.method!=='POST'||!match||req.headers.origin!==`http://127.0.0.1:${server.address().port}`){res.writeHead(403).end();return;}
  const chunks=[];let size=0;for await(const chunk of req){size+=chunk.length;if(size>2_100_000)throw Error('Export too large');chunks.push(chunk);}
  const bytes=Buffer.concat(chunks);
  if(match[1]==='error.txt'){completed.reject(Error(bytes.toString().slice(0,1000)));res.writeHead(200).end();return;}
  if(match[1].endsWith('.jpg')&&(bytes.length<1000||bytes.length>2_000_000||bytes[0]!==0xff||bytes[1]!==0xd8))throw Error('Invalid JPEG export');
  await fs.writeFile(path.join(output,match[1]),bytes,{mode:0o600});
  res.writeHead(200).end('saved');
  if(match[1]==='verification.json')completed.resolve();
 }catch(error){res.writeHead(500).end('render error');if(req.method==='POST')completed.reject(error);}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const profile=await fs.mkdtemp(path.join(os.tmpdir(),'adu-scorecard-'));
const url=`http://127.0.0.1:${server.address().port}/render-live.html`;
const child=spawn(browserPath,['--headless=new','--no-sandbox','--disable-dev-shm-usage','--enable-webgl','--use-gl=angle','--use-angle=swiftshader','--enable-unsafe-swiftshader',`--user-data-dir=${profile}`,url],{stdio:['ignore','ignore','pipe']});
let stderr='';child.stderr.on('data',chunk=>{stderr=(stderr+chunk.toString()).slice(-2000);});
let timer;
try{
 await Promise.race([done,new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error(`Chrome render timed out: ${stderr}`)),120000);}),new Promise((_,reject)=>child.on('error',reject)),new Promise((_,reject)=>child.on('exit',(code,signal)=>reject(Error(`Chrome exited before rendering (${code??signal}): ${stderr}`))))]);
 const verification=JSON.parse(await fs.readFile(path.join(output,'verification.json')));
 if(verification.renders?.length!==2||verification.source_run!==snapshot.source_run||verification.focus!==snapshot.focus.id)throw Error('Render verification mismatch');
 process.stdout.write(JSON.stringify(verification)+'\n');
}finally{
 clearTimeout(timer);
 if(child.exitCode===null){
  child.kill('SIGTERM');
  await Promise.race([new Promise(resolve=>child.once('exit',resolve)),new Promise(resolve=>setTimeout(resolve,3000))]);
  if(child.exitCode===null)child.kill('SIGKILL');
 }
 server.close();await fs.rm(profile,{recursive:true,force:true,maxRetries:5,retryDelay:200});
}
