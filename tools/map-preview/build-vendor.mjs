// MapLibre 6 ships ES modules. Package its main thread and worker into the
// existing single vendor asset so the updater's release contract stays stable.
import {build} from 'esbuild';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=path.dirname(fileURLToPath(import.meta.url));
const dist=path.join(root,'node_modules/maplibre-gl/dist');
const worker=await build({entryPoints:[path.join(dist,'maplibre-gl-worker.mjs')],bundle:true,write:false,format:'iife',minify:true,platform:'browser',target:'es2022'});
const main=await build({entryPoints:[path.join(dist,'maplibre-gl.mjs')],bundle:true,write:false,format:'iife',globalName:'maplibregl',minify:true,platform:'browser',target:'es2022',define:{'import.meta.url':'globalThis.location.href'}});
const js=main.outputFiles[0].text+'\nmaplibregl.setWorkerUrl(URL.createObjectURL(new Blob(['+JSON.stringify(worker.outputFiles[0].text)+'],{type:"text/javascript"})));\n';
await fs.writeFile(path.join(dist,'maplibre-gl.js'),js);
