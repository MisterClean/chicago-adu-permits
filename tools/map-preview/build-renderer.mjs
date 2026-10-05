// Preserve the updater's seven-asset contract: bundle the controller dependencies
// into render-live.mjs; no npm install is needed on the production host.
import {build} from 'esbuild';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=path.dirname(fileURLToPath(import.meta.url));
await import('./build-vendor.mjs');
const outfile=process.argv[2];
if(!outfile)throw Error('Usage: node build-renderer.mjs OUTPUT.mjs');
await build({entryPoints:[path.join(root,'render-live.mjs')],outfile,bundle:true,platform:'node',format:'esm',target:'node22',banner:{js:"import {createRequire as aduCreateRequire} from 'node:module';const require=aduCreateRequire(import.meta.url);"}});
