import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {apiFailure,closeOwnedSession} from './render-cloudflare.mjs';
test('Cloudflare credentials are held independently of Bluesky',()=>{
 for(const status of [401,403])assert.equal(apiFailure(status,'','').category,'auth');
});
test('daily quota and acquisition limits have distinct retries',()=>{
 const quota=apiFailure(429,'daily browser time quota exhausted','20');
 assert.equal(quota.category,'quota');assert.ok(quota.retry_after_seconds>0&&quota.retry_after_seconds<=86400);
 assert.deepEqual(apiFailure(429,'too many concurrent browsers','35'),{category:'rate_limit',retry_after_seconds:35});
 const date=new Date(Date.now()+75000).toUTCString();
 assert.ok(apiFailure(429,'rate limit',date).retry_after_seconds>=73);
 assert.equal(apiFailure(429,'rate limit','999999').retry_after_seconds,86400);
 assert.equal(apiFailure(503,'unavailable','').category,'transport');
});

test('a failed CDP close still deletes only the owned session',async()=>{
 const dir=await fs.mkdtemp(path.join(os.tmpdir(),'adu-render-test-'));
 try{
  const file=path.join(dir,'active.json');await fs.writeFile(file,'owned');
  const calls=[];let disconnected=false;
  const browser={close:async()=>{throw Error('disconnect');},disconnect:()=>{disconnected=true;}};
  await closeOwnedSession(browser,'owned-session','https://cf.invalid/browser',file,async(...args)=>{calls.push(args);});
  assert.equal(disconnected,true);
  assert.deepEqual(calls,[['https://cf.invalid/browser/owned-session','DELETE',true]]);
  await assert.rejects(fs.stat(file),{code:'ENOENT'});
  await fs.writeFile(file,'owned');
  await assert.rejects(closeOwnedSession(null,'owned-session','https://cf.invalid/browser',file,async()=>{throw Error('network');}));
  assert.equal(await fs.readFile(file,'utf8'),'owned');
 }finally{await fs.rm(dir,{recursive:true,force:true});}
});
test('a hanging CDP close has a deadline and still performs HTTP cleanup',async()=>{
 let disconnected=false,deleted=false;
 await closeOwnedSession({close:()=>new Promise(()=>{}),disconnect:()=>{disconnected=true;}},'owned-session','https://cf.invalid/browser',null,async()=>{deleted=true;},10);
 assert.ok(disconnected&&deleted);
});
