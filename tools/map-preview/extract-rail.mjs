import {VectorTile} from '@mapbox/vector-tile';
import {PbfReader as Pbf} from 'pbf';
import fs from 'node:fs/promises';
const root=new URL('./data/',import.meta.url);
const b=JSON.parse(await fs.readFile(new URL('tilejson.json',root)));
const url=b.tiles[0],z=14;
const tile=(lng,lat)=>[(lng+180)/360*2**z,(1-Math.asinh(Math.tan(lat*Math.PI/180))/Math.PI)/2*2**z];
const nw=tile(-87.68,41.94),se=tile(-87.61,41.895),features=[];let counts={};
for(let x=Math.floor(nw[0]);x<=Math.floor(se[0]);x++)for(let y=Math.floor(nw[1]);y<=Math.floor(se[1]);y++){
 const u=url.replace('{z}',z).replace('{x}',x).replace('{y}',y);
 const r=await fetch(u);if(!r.ok)throw Error(r.status+' tile');
 const buffer=new Uint8Array(await r.arrayBuffer());await fs.writeFile(new URL(`tile-${z}-${x}-${y}.pbf`,root),buffer);
 const tiledata=new VectorTile(new Pbf(buffer));const layer=tiledata.layers.transportation;
 if(!layer)continue;
 for(let i=0;i<layer.length;i++){const f=layer.feature(i);const cl=f.properties.class;counts[cl]=(counts[cl]||0)+1;if(['rail','transit'].includes(cl)){features.push(f.toGeoJSON(x,y,z));}}
}
await fs.writeFile(new URL('rail.json',root),JSON.stringify({type:'FeatureCollection',features}));console.log(features.length,'rail segments',counts);console.log(features.slice(0,10).map(f=>f.properties));
