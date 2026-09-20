import fs from 'node:fs/promises';
import {VectorTile} from '@mapbox/vector-tile';
import {PbfReader} from 'pbf';
const root=new URL('./data/',import.meta.url),features=[],seen=new Set();
const focus=JSON.parse(await fs.readFile(new URL('points.json',root))).features.find(f=>f.properties.id==='954542').geometry.coordinates;
const food=new Set(['restaurant','cafe','fast_food','bakery','ice_cream','bar','beer']);
const shops=new Set(['shop','clothing_store','grocery','supermarket','bicycle','books','convenience','furniture']);
for(const file of (await fs.readdir(root)).filter(f=>/^tile-14-.*pbf$/.test(f))){
 const [z,x,y]=file.match(/\d+/g).map(Number),layer=new VectorTile(new PbfReader(await fs.readFile(new URL(file,root)))).layers.poi;
 if(!layer)continue;
 for(let i=0;i<layer.length;i++){
  const f=layer.feature(i),g=f.toGeoJSON(x,y,z),p=f.properties,name=p.name_en||p['name:en']||p.name;
  if(!name||g.geometry.type!=='Point'||(!food.has(p.class)&&!shops.has(p.class)))continue;
  const c=g.geometry.coordinates,d=Math.hypot((c[0]-focus[0])*83000,(c[1]-focus[1])*111320);
  if(d>650)continue;
  const normalized=name.toLocaleLowerCase().replace(/^the\s+/,'').replace(/[^\p{L}\p{N}]/gu,'');
  const key=normalized+':'+c.map(n=>n.toFixed(4)).join(',');if(seen.has(key))continue;seen.add(key);
  features.push({type:'Feature',id:features.length,geometry:g.geometry,properties:{name,category:shops.has(p.class)?'shop':p.class==='cafe'?'cafe':'food',kind:p.subclass,distance_m:Math.round(d),rank:Number(p.rank)||30}});
 }
}
features.sort((a,b)=>a.properties.distance_m-b.properties.distance_m);
// Keep one mapped feature when the same named business appears twice within 15 m.
const unique=[];
for(const f of features){let name=f.properties.name.toLowerCase().replace(/^the\s+/,'').replace(/[^\p{L}\p{N}]/gu,'');if(unique.some(g=>g.properties.name.toLowerCase().replace(/^the\s+/,'').replace(/[^\p{L}\p{N}]/gu,'')===name&&Math.hypot((f.geometry.coordinates[0]-g.geometry.coordinates[0])*83000,(f.geometry.coordinates[1]-g.geometry.coordinates[1])*111320)<15))continue;unique.push(f);}
await fs.writeFile(new URL('poi.json',root),JSON.stringify({type:'FeatureCollection',features:unique}));
console.log(unique.length,'nearby POIs',unique.slice(0,20).map(f=>({name:f.properties.name,kind:f.properties.kind,distance:f.properties.distance_m})));
