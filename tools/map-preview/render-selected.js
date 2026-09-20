const BLUE='#41b6e6', RED='#e4002b';
const params=new URLSearchParams(location.search);
const themes=[
 {id:'n1',name:'Civic model',bg:'#e9f0f3',road:'#ffffff',case:'#c8d4dc',building:'#acd2e4',ink:'#253c4b',park:'#dbe7de',water:'#86cce6',rail:'#607b8c',pitch:54,bearing:-24},
 {id:'n2',name:'Midnight',bg:'#121a22',road:'#293842',case:'#354b5b',building:'#477086',ink:'#d4e4ec',park:'#1d3838',water:'#15374a',rail:'#849da9',pitch:58,bearing:-24,dark:true},
 {id:'n3',name:'Blueprint',bg:'#133f64',road:'#326484',case:'#447b9d',building:'#a0d9f0',ink:'#e2f5ff',park:'#22577a',water:'#0d3554',rail:'#69b1d8',pitch:48,bearing:0,dark:true},
 {id:'n4',name:'Warm city',bg:'#ede7db',road:'#fffdf7',case:'#d9d0c1',building:'#c8ae94',ink:'#5c5144',park:'#d6dfc7',water:'#9bc8d5',rail:'#958778',pitch:60,bearing:25},
 {id:'n5',name:'Architect’s model',bg:'#e0e2e3',road:'#fafafa',case:'#c5c8ca',building:'#f3f4f4',ink:'#3b4348',park:'#d1d7d2',water:'#b1cbd6',rail:'#798185',pitch:62,bearing:-42},
 {id:'wa',name:'Civic atlas',bg:'#eff3f5',road:'#ffffff',case:'#d8e1e5',building:'#dbe5e9',ink:'#455863',park:'#e0ebe4',water:'#b6ddeb',rail:'#727f8e'},
 {id:'wb',name:'Night atlas',bg:'#111b25',road:'#2b3c4b',case:'#334a5c',building:'#1e3140',ink:'#acbecb',park:'#193735',water:'#163b50',rail:'#8ca2b3',dark:true},
 {id:'wc',name:'Transit atlas',bg:'#f4f2ed',road:'#ffffff',case:'#d8d6d0',building:'#e3e1db',ink:'#66645d',park:'#e4e9d7',water:'#b0d9e5',rail:'#326884'}
];
const get=async x=>(await fetch('data/'+x+'.json')).json();
const [base,points,boundary,summary,rail,poi]=await Promise.all(['base-style','points','ward43','summary','rail','poi'].map(get));
const focus=points.features.find(x=>x.properties.id==='954542');
const center=focus.geometry.coordinates;
const bbox=[Infinity,Infinity,-Infinity,-Infinity];
function coords(x){if(typeof x[0]==='number'){bbox[0]=Math.min(bbox[0],x[0]);bbox[1]=Math.min(bbox[1],x[1]);bbox[2]=Math.max(bbox[2],x[0]);bbox[3]=Math.max(bbox[3],x[1]);}else x.forEach(coords);}
boundary.features.forEach(x=>coords(x.geometry.coordinates));
const metadata={summary,center,ward_bbox:bbox,renders:[]};
await document.fonts.load('bold 80px Big');await document.fonts.load('20px Roboto');
function style(t,ward){
 const s=structuredClone(base);s.name=t.name;
 s.layers=s.layers.filter(l=>!l.id.startsWith('boundary')&&!l.id.startsWith('label_')&&!l.id.includes('shield')&&!l.id.includes('airport'));
 for(const l of s.layers){
  const p=l.paint??={};
  if(l.type==='background')p['background-color']=t.bg;
  if(l.type==='fill'){
   p['fill-color']=l.id.includes('water')?t.water:l.id.includes('building')?t.building:l.id==='park'||l.id.includes('wood')?t.park:t.bg;
   p['fill-opacity']=1;
  }
  if(l.type==='line'){
   p['line-color']=l.id.includes('water')?t.water:l.id.includes('railway')?t.rail:l.id.includes('casing')?t.case:t.road;
   if(l.id.includes('railway'))l.layout={...l.layout,visibility:'none'};
  }
  if(l.type==='symbol'){
   p['text-color']=t.ink;p['text-halo-color']=t.bg;p['text-halo-width']=1.6;
   l.layout['text-size']=ward?14:16;
   if(l.id==='highway-name-minor')l.minzoom=ward?13.7:15;
  }
 }
 const firstLabel=s.layers.findIndex(l=>l.type==='symbol');
 const extra=[{id:'rail-context',type:'line',source:'openmaptiles','source-layer':'transportation',filter:['match',['get','class'],['rail','transit'],true,false],paint:{'line-color':t.rail,'line-width':t.id==='wc'?6:ward?3.5:3.8,'line-opacity':1}}];
 if(!ward)extra.push({id:'buildings-3d',type:'fill-extrusion',source:'openmaptiles','source-layer':'building',filter:['!=',['get','hide_3d'],true],paint:{'fill-extrusion-color':t.building,'fill-extrusion-height':['coalesce',['get','render_height'],8],'fill-extrusion-base':['coalesce',['get','render_min_height'],0],'fill-extrusion-opacity':1,'fill-extrusion-vertical-gradient':true}});
 s.layers.splice(firstLabel,0,...extra);
 s.light={anchor:'viewport',color:'#ffffff',intensity:t.id==='n5'?.5:.34,position:[1.5,210,48]};
 return s;
}
function star(ctx,x,y,r){ctx.beginPath();for(let i=0;i<12;i++){let a=-Math.PI/2+i*Math.PI/6;let d=i%2?r*.36:r;ctx.lineTo(x+Math.cos(a)*d,y+Math.sin(a)*d)}ctx.closePath();ctx.fill();}
function text(ctx,s,x,y,size,color='#fff',font='Roboto'){ctx.fillStyle=color;ctx.font=`${size}px ${font}`;ctx.fillText(s,x,y);}
function pin(ctx,x,y,color=RED){ctx.save();ctx.translate(x,y);ctx.shadowColor='#0005';ctx.shadowBlur=12;ctx.fillStyle=color;ctx.beginPath();ctx.moveTo(0,0);ctx.bezierCurveTo(-6,-13,-20,-27,-20,-39);ctx.arc(0,-39,20,Math.PI,0);ctx.bezierCurveTo(20,-27,6,-13,0,0);ctx.fill();ctx.shadowBlur=0;ctx.fillStyle='white';ctx.beginPath();ctx.arc(0,-39,7,0,7);ctx.fill();ctx.restore();}
function dot(ctx,x,y,n,t,outline=false){let r=7+Math.sqrt(n)*5;ctx.fillStyle=t.dark?(n>=4?'#ade5ff':n>=2?'#5ca9cc':'#367895'):(n>=4?'#075f91':n>=2?'#398cac':'#76c6e5');ctx.strokeStyle=t.dark?'#b9eaff':'#193d50';ctx.lineWidth=1.7;ctx.beginPath();ctx.arc(x,y,r,0,7);ctx.fill();ctx.stroke();text(ctx,String(n),x-(n>=10?9:4.5),y+5,15,t.dark?(n>=2?'#102e40':'white'):(n>1?'white':'#102e40'));if(outline){ctx.strokeStyle=RED;ctx.lineWidth=4;ctx.beginPath();ctx.arc(x,y,r+8,0,7);ctx.stroke();}}
const WIDTH=1080,HEIGHT=1080,MAP_TOP=194;
function waitIdle(map){return new Promise((resolve,reject)=>{const timeout=setTimeout(()=>reject(Error('Map did not finish loading')),35000);map.once('idle',()=>{clearTimeout(timeout);resolve();});map.triggerRepaint();});}
function poiIcon(kind){
 const size=48,c=document.createElement('canvas');c.width=c.height=size;const x=c.getContext('2d');
 x.fillStyle=kind==='shop'?'#265d78':kind==='cafe'?'#655039':'#84603e';x.beginPath();x.arc(24,24,22,0,Math.PI*2);x.fill();x.strokeStyle='#fff';x.lineWidth=2.5;x.stroke();x.lineJoin='round';x.lineCap='round';
 if(kind==='shop'){x.strokeRect(15,20,18,15);x.beginPath();x.arc(24,20,5,Math.PI,0);x.stroke();}
 else if(kind==='cafe'){x.strokeRect(14,19,16,12);x.beginPath();x.arc(31,24,5,-Math.PI/2,Math.PI/2);x.moveTo(13,35);x.lineTo(34,35);x.stroke();}
 else{x.beginPath();for(const dx of [14,18,22]){x.moveTo(dx,13);x.lineTo(dx,22);}x.moveTo(14,22);x.lineTo(22,22);x.moveTo(18,22);x.lineTo(18,35);x.moveTo(31,13);x.lineTo(31,35);x.moveTo(31,13);x.quadraticCurveTo(25,18,31,25);x.stroke();}
 return {width:size,height:size,data:x.getImageData(0,0,size,size).data};
}
function spreadApplicationLabels(map){
 const labels=points.features.map(f=>{const p=map.project(f.geometry.coordinates);return {f,p,x:p.x,y:p.y,r:7+Math.sqrt(f.properties.quantity)*5+(f.properties.id===focus.properties.id?8:0)};});
 for(let pass=0;pass<32;pass++)for(let i=0;i<labels.length;i++)for(let j=i+1;j<labels.length;j++){
  const a=labels[i],b=labels[j],dx=b.x-a.x,dy=b.y-a.y,d=Math.hypot(dx,dy),gap=a.r+b.r+7;
  if(d>=gap)continue;const ux=d>.1?dx/d:1,uy=d>.1?dy/d:0,move=(gap-d)/2+.15;
  if(a.f.properties.id!==focus.properties.id){a.x-=ux*move;a.y-=uy*move;}
  if(b.f.properties.id!==focus.properties.id){b.x+=ux*move;b.y+=uy*move;}
 }
 return labels;
}
async function renderSelected(t){
 const ward=t.id==='wc',mapHeight=ward?800:848;
 document.getElementById('map').style.height=mapHeight+'px';
 document.getElementById('status').textContent=ward?'Rendering ward map…':'Rendering neighborhood map…';
 const map=new maplibregl.Map({container:'map',style:style(t,ward),center,zoom:16.58,pitch:ward?0:60,bearing:ward?0:t.bearing,interactive:false,attributionControl:false,canvasContextAttributes:{preserveDrawingBuffer:true,antialias:true},pixelRatio:2});
 const errors=[];map.on('error',e=>errors.push(e.error?.message||String(e)));
 await new Promise((resolve,reject)=>{map.once('load',resolve);setTimeout(()=>{if(!map.loaded())reject(Error('Map failed to load'));},35000);});
 if(ward){
  map.fitBounds([[bbox[0],bbox[1]],[bbox[2],bbox[3]]],{padding:{top:44,bottom:54,left:76,right:76},duration:0});
  map.addSource('ward',{type:'geojson',data:boundary});
  const rings=boundary.features.flatMap(f=>f.geometry.type==='MultiPolygon'?f.geometry.coordinates.map(p=>p[0]):[f.geometry.coordinates[0]]);
  map.addSource('outside',{type:'geojson',data:{type:'Feature',properties:{},geometry:{type:'Polygon',coordinates:[[[-180,-85],[180,-85],[180,85],[-180,85],[-180,-85]],...rings]}}});
  const labelLayer=map.getStyle().layers.find(x=>x.type==='symbol')?.id;
  map.addLayer({id:'outside-fade',type:'fill',source:'outside',paint:{'fill-color':t.bg,'fill-opacity':.70}},labelLayer);
  map.addLayer({id:'ward-fill',type:'fill',source:'ward',paint:{'fill-color':BLUE,'fill-opacity':.035}},labelLayer);
  map.addLayer({id:'ward-border-halo',type:'line',source:'ward',paint:{'line-color':'#fff','line-width':8}});
  map.addLayer({id:'ward-border',type:'line',source:'ward',paint:{'line-color':'#183e56','line-width':3.6}});
  map.removeLayer('rail-context');map.addSource('detailed-rail',{type:'geojson',data:rail});
  map.addLayer({id:'rail-context',type:'line',source:'detailed-rail',layout:{'line-join':'round','line-cap':'round'},paint:{'line-color':t.rail,'line-width':4.3,'line-opacity':.85}});
 }else map.moveLayer('rail-context');
 map.addLayer({id:'station-dots',type:'circle',source:'openmaptiles','source-layer':'poi',filter:['any',['==',['get','class'],'railway'],['==',['get','subclass'],'station']],paint:{'circle-color':'#fff','circle-radius':ward?5:6,'circle-stroke-color':t.rail,'circle-stroke-width':2.5}});
 map.addLayer({id:'station-names',type:'symbol',source:'openmaptiles','source-layer':'poi',filter:['any',['==',['get','class'],'railway'],['==',['get','subclass'],'station']],layout:{'text-field':['coalesce',['get','name_en'],['get','name']],'text-font':['Noto Sans Regular'],'text-size':ward?15:19,'text-offset':[0,1.15],'text-anchor':'top'},paint:{'text-color':t.ink,'text-halo-color':t.bg,'text-halo-width':2}});
 if(!ward){
  map.setLayoutProperty('station-names','text-anchor',['case',['==',['get','name'],'Fullerton'],'top-right','top']);
  for(const cat of ['food','shop','cafe'])map.addImage('poi-'+cat,poiIcon(cat),{pixelRatio:2});
  const visiblePois={...poi,features:poi.features.filter(f=>{const p=map.project(f.geometry.coordinates);return p.x>=100&&p.x<=980&&p.y>=70&&p.y<=mapHeight-75;})};
  map.addSource('nearby-places',{type:'geojson',data:visiblePois});
  map.addLayer({id:'nearby-places',type:'symbol',source:'nearby-places',filter:['<=',['get','distance_m'],520],layout:{'symbol-sort-key':['*',['get','distance_m'],['case',['==',['get','category'],'shop'],0.65,1]],'icon-image':['concat','poi-',['get','category']],'icon-size':.82,'text-field':['get','name'],'text-font':['Noto Sans Regular'],'text-size':18,'text-variable-anchor':['top','bottom'],'text-radial-offset':1,'text-justify':'auto','text-max-width':9,'text-line-height':1.13,'text-padding':9,'icon-padding':8,'text-optional':false,'icon-optional':false},paint:{'text-color':'#233943','text-halo-color':'#ffffff','text-halo-width':2,'text-halo-blur':.3}});
  map.moveLayer('station-names');
 }
 await waitIdle(map);
 const c=document.createElement('canvas');c.width=2160;c.height=2160;c.className='output';const ctx=c.getContext('2d');ctx.scale(2,2);
 ctx.fillStyle='#000';ctx.fillRect(0,0,WIDTH,HEIGHT);ctx.fillStyle=BLUE;ctx.fillRect(0,0,WIDTH,8);ctx.fillStyle=RED;for(let i=0;i<4;i++)star(ctx,874+i*47,44,16);
 text(ctx,ward?'THE WARD SO FAR':'AROUND THE PREAPPROVAL',42,55,21,BLUE);
 text(ctx,ward?'WARD 43':'2057 N SHEFFIELD AVE',42,133,ward?83:65,'white','Big');
 text(ctx,ward?'16 ADUs  /  13 APPLICATIONS  /  TIED #11 OF 50':'1 ADU REQUESTED   /   WARD 43',43,171,ward?24:23,BLUE);
 ctx.drawImage(map.getCanvas(),0,MAP_TOP,WIDTH,mapHeight);
 let labelEvidence=[];
 if(ward){
  const labels=spreadApplicationLabels(map);labelEvidence=labels.map(({f,p,x,y})=>({id:f.properties.id,anchor:p,label:{x,y}}));
  for(const {f,p,x,y} of labels){if(f.properties.id===focus.properties.id)continue;
   if(Math.hypot(x-p.x,y-p.y)>4){ctx.strokeStyle='#354f60';ctx.lineWidth=1.5;ctx.beginPath();ctx.moveTo(p.x,p.y+MAP_TOP);ctx.lineTo(x,y+MAP_TOP);ctx.stroke();ctx.fillStyle='#17364a';ctx.beginPath();ctx.arc(p.x,p.y+MAP_TOP,2.5,0,7);ctx.fill();}
   dot(ctx,x,y+MAP_TOP,f.properties.quantity,t);
  }
  const p=map.project(center);dot(ctx,p.x,p.y+MAP_TOP,1,t,true);
  ctx.strokeStyle='#b1c4ce';ctx.lineWidth=1;ctx.strokeRect(20,MAP_TOP+20,1040,mapHeight-40);
 }else{const p=map.project(center);pin(ctx,p.x,p.y+MAP_TOP);}
 ctx.save();ctx.translate(1024,MAP_TOP+54);ctx.fillStyle='#fffffff0';ctx.beginPath();ctx.arc(0,0,27,0,7);ctx.fill();ctx.rotate(-map.getBearing()*Math.PI/180);ctx.fillStyle='#243c4a';ctx.beginPath();ctx.moveTo(0,-18);ctx.lineTo(-6,3);ctx.lineTo(6,3);ctx.closePath();ctx.fill();text(ctx,'N',-5,19,13,'#243c4a');ctx.restore();
 if(ward){
  text(ctx,'Application submitted April 1 2026 - September 19, 2026',42,1027,19,'#cad1d5');
 }
 ctx.save();ctx.textAlign='right';text(ctx,'Data: City of Chicago Data Portal · © OpenMapTiles · © OpenStreetMap contributors',1038,1061,12,'#c1c9cc');ctx.restore();ctx.fillStyle=BLUE;ctx.fillRect(0,1072,1080,8);
 let q=.94,blob=await new Promise(resolve=>c.toBlob(resolve,'image/jpeg',q));while(blob.size>1950000&&q>.5){q-=.04;blob=await new Promise(resolve=>c.toBlob(resolve,'image/jpeg',q));}
 const name=t.id+'-selected';await fetch('/export/'+name+'.jpg',{method:'POST',body:blob});
 const nearby=ward?[]:map.queryRenderedFeatures({layers:['nearby-places']}).map(f=>f.properties);
 const bounds=ward?[map.project([bbox[0],bbox[1]]),map.project([bbox[2],bbox[3]])]:null;
 metadata.renders.push({id:name,bytes:blob.size,width:c.width,height:c.height,zoom:map.getZoom(),center:map.getCenter(),bearing:map.getBearing(),pitch:map.getPitch(),poi:nearby,stations:map.queryRenderedFeatures({layers:['station-names']}).map(f=>f.properties.name),wardScreenBounds:bounds,mapHeight,labelEvidence,errors});
 map.remove();document.body.append(c);
}
for(const id of ['n5','wc'])await renderSelected(themes.find(t=>t.id===id));
await fetch('/export/selected-verification.json',{method:'POST',body:JSON.stringify(metadata,null,2)});
document.getElementById('status').textContent='Selected maps complete';document.getElementById('map').remove();
