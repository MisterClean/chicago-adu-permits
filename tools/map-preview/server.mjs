import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.MAP_PREVIEW_PORT || 4318);
const origin = `http://127.0.0.1:${port}`;
const fonts = path.resolve(root, '../../assets/fonts');
const types = {'.html':'text/html', '.js':'text/javascript', '.json':'application/json', '.css':'text/css', '.jpg':'image/jpeg', '.png':'image/png', '.ttf':'font/ttf'};
const shared = new Map([
  ['/vendor/maplibre-gl.js', path.join(root, 'node_modules/maplibre-gl/dist/maplibre-gl.js')],
  ['/vendor/maplibre-gl.css', path.join(root, 'node_modules/maplibre-gl/dist/maplibre-gl.css')],
  ['/fonts/BigShouldersText-Bold.ttf', path.join(fonts, 'BigShouldersText-Bold.ttf')],
  ['/fonts/Roboto.ttf', path.join(fonts, 'Roboto.ttf')],
]);

await fs.mkdir(path.join(root, 'renders'), {recursive:true});
http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, origin);
    if (req.method === 'POST' && /^\/export\/[a-z0-9-]+\.(jpg|json)$/.test(url.pathname)) {
      if (req.headers.origin !== origin) {
        res.writeHead(403).end('Only same-origin exports are accepted');
        return;
      }
      const chunks = [];
      let length = 0;
      for await (const chunk of req) {
        length += chunk.length;
        if (length > 12_000_000) {
          res.writeHead(413).end('Export too large');
          return;
        }
        chunks.push(chunk);
      }
      await fs.writeFile(path.join(root, 'renders', path.basename(url.pathname)), Buffer.concat(chunks));
      res.end('saved');
      return;
    }
    if (req.method !== 'GET') {
      res.writeHead(405).end('Method not allowed');
      return;
    }
    const rel = decodeURIComponent(url.pathname === '/' ? '/index.html' : url.pathname);
    const target = shared.get(rel) || path.resolve(root, '.' + rel);
    if (!shared.has(rel) && !target.startsWith(root + path.sep)) {
      res.writeHead(403).end('Invalid path');
      return;
    }
    const data = await fs.readFile(target);
    res.setHeader('Content-Type', types[path.extname(target)] || 'application/octet-stream');
    res.setHeader('Cache-Control', 'no-store');
    res.end(data);
  } catch {
    res.writeHead(404).end('Not found');
  }
}).listen(port, '127.0.0.1', () => console.log(`Map review at ${origin}`));
