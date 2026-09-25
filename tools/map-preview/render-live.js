const BLUE='#41b6e6',RED='#e4002b';
const [snapshot,base]=await Promise.all([fetch('/payload.json').then(r=>r.json()),fetch('/data/base-style.json').then(r=>r.json())]);
const center=[snapshot.focus.location.longitude,snapshot.focus.location.latitude];
const points=snapshot.points;
const permit=snapshot.mode==='permit';
const boundary=snapshot.boundary;
const bounds=[Infinity,Infinity,-Infinity,-Infinity];
function visit(c){if(typeof c[0]==='number'){bounds[0]=Math.min(bounds[0],c[0]);bounds[1]=Math.min(bounds[1],c[1]);bounds[2]=Math.max(bounds[2],c[0]);bounds[3]=Math.max(bounds[3],c[1]);}else c.forEach(visit);}
boundary.features.forEach(f=>visit(f.geometry.coordinates));
if(!bounds.every(Number.isFinite)||points.length<1)throw Error('Invalid map evidence');
await document.fonts.load('bold 80px Big');await document.fonts.load('20px Roboto');
const themes={n5:{bg:'#e0e2e3',road:'#fafafa',case:'#c5c8ca',building:'#f3f4f4',ink:'#3b4348',park:'#d1d7d2',water:'#b1cbd6',rail:'#798185'},wc:{bg:'#f4f2ed',road:'#fff',case:'#d8d6d0',building:'#e3e1db',ink:'#66645d',park:'#e4e9d7',water:'#b0d9e5',rail:'#326884'}};
function style(t,ward){
 const s=structuredClone(base);
 s.layers=s.layers.filter(l=>!l.id.startsWith('boundary')&&!l.id.startsWith('label_')&&!l.id.includes('shield')&&!l.id.includes('airport'));
 for(const l of s.layers){
  const p=l.paint??={};
  if(l.type==='background')p['background-color']=t.bg;
  if(l.type==='fill'){p['fill-color']=l.id.includes('water')?t.water:l.id.includes('building')?t.building:l.id.includes('park')||l.id.includes('wood')?t.park:t.bg;p['fill-opacity']=1;}
  if(l.type==='line'){p['line-color']=l.id.includes('water')?t.water:l.id.includes('railway')?t.rail:l.id.includes('casing')?t.case:t.road;if(l.id.includes('railway'))l.layout={...l.layout,visibility:'none'};}
  if(l.type==='symbol'){p['text-color']=t.ink;p['text-halo-color']=t.bg;p['text-halo-width']=1.6;l.layout['text-size']=ward?14:16;if(l.id==='highway-name-minor')l.minzoom=ward?13.7:15;}
 }
 const first=s.layers.findIndex(l=>l.type==='symbol');
 const extra=[{id:'rail-context',type:'line',source:'openmaptiles','source-layer':'transportation',filter:['match',['get','class'],['rail','transit'],true,false],paint:{'line-color':t.rail,'line-width':ward?4.3:3.8,'line-opacity':.85}}];
 if(!ward)extra.push({id:'buildings-3d',type:'fill-extrusion',source:'openmaptiles','source-layer':'building',filter:['!=',['get','hide_3d'],true],paint:{'fill-extrusion-color':t.building,'fill-extrusion-height':['coalesce',['get','render_height'],8],'fill-extrusion-base':['coalesce',['get','render_min_height'],0],'fill-extrusion-opacity':1}});
 s.layers.splice(first,0,...extra);
 s.light={anchor:'viewport',color:'#fff',intensity:.5,position:[1.5,210,48]};
 return s;
}
function star(ctx,x,y,r){ctx.beginPath();for(let i=0;i<12;i++){const a=-Math.PI/2+i*Math.PI/6,d=i%2?r*.36:r;ctx.lineTo(x+Math.cos(a)*d,y+Math.sin(a)*d);}ctx.closePath();ctx.fill();}
function label(ctx,s,x,y,size,color='#fff',font='Roboto'){ctx.fillStyle=color;ctx.font=`${size}px ${font}`;ctx.fillText(s,x,y);}
function fitted(ctx,s,x,y,max,size,color='#fff',font='Big'){ctx.font=`${size}px ${font}`;while(ctx.measureText(s).width>max&&size>28){size-=2;ctx.font=`${size}px ${font}`;}label(ctx,s,x,y,size,color,font);}
function pin(ctx,x,y){ctx.save();ctx.translate(x,y);ctx.shadowColor='#0005';ctx.shadowBlur=12;ctx.fillStyle=RED;ctx.beginPath();ctx.moveTo(0,0);ctx.bezierCurveTo(-6,-13,-20,-27,-20,-39);ctx.arc(0,-39,20,Math.PI,0);ctx.bezierCurveTo(20,-27,6,-13,0,0);ctx.fill();ctx.shadowBlur=0;ctx.fillStyle='white';ctx.beginPath();ctx.arc(0,-39,7,0,7);ctx.fill();ctx.restore();}
function dot(ctx,x,y,n,focus){const r=7+Math.sqrt(n)*5;ctx.fillStyle=n>=4?'#075f91':n>=2?'#398cac':'#76c6e5';ctx.strokeStyle='#193d50';ctx.lineWidth=1.7;ctx.beginPath();ctx.arc(x,y,r,0,7);ctx.fill();ctx.stroke();ctx.textAlign='center';label(ctx,String(n),x,y+5,15,n>1?'white':'#102e40');ctx.textAlign='start';if(focus){ctx.strokeStyle=RED;ctx.lineWidth=4;ctx.beginPath();ctx.arc(x,y,r+8,0,7);ctx.stroke();}}
function waitIdle(map){return new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Map tiles timed out')),45000);map.once('idle',()=>{clearTimeout(timer);resolve();});map.triggerRepaint();});}
function positions(map){
 const items=points.map(f=>{const p=map.project([f.location.longitude,f.location.latitude]);return {f,p,x:p.x,y:p.y,r:7+Math.sqrt(f.quantity)*5+(f.id===snapshot.focus.id?8:0)};});
 for(let pass=0;pass<32;pass++)for(let i=0;i<items.length;i++)for(let j=i+1;j<items.length;j++){
  const a=items[i],b=items[j],dx=b.x-a.x,dy=b.y-a.y,d=Math.hypot(dx,dy),gap=a.r+b.r+7;
  if(d>=gap)continue;const ux=d>.1?dx/d:1,uy=d>.1?dy/d:0,move=(gap-d)/2+.15;
  if(a.f.id!==snapshot.focus.id){a.x-=ux*move;a.y-=uy*move;}
  if(b.f.id!==snapshot.focus.id){b.x+=ux*move;b.y+=uy*move;}
 }
 return items;
}
const results=[];
async function render(id){
 const ward=id==='wc',t=themes[id],height=ward?800:848;
 document.querySelector('#map').style.height=height+'px';
 document.querySelector('#status').textContent=`Rendering ${id}`;
 const map=new maplibregl.Map({container:'map',style:style(t,ward),center,zoom:16.58,pitch:ward?0:60,bearing:ward?0:-42,interactive:false,attributionControl:false,canvasContextAttributes:{preserveDrawingBuffer:true,antialias:true},pixelRatio:2});
 const errors=[];map.on('error',e=>errors.push(e.error?.message||String(e)));
 await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Map style timed out')),45000);map.once('load',()=>{clearTimeout(timer);resolve();});});
 if(ward){
  map.fitBounds([[bounds[0],bounds[1]],[bounds[2],bounds[3]]],{padding:{top:44,bottom:54,left:76,right:76},duration:0});
  map.addSource('ward',{type:'geojson',data:boundary});
  const rings=boundary.features.flatMap(f=>f.geometry.type==='MultiPolygon'?f.geometry.coordinates.map(p=>p[0]):[f.geometry.coordinates[0]]);
  map.addSource('outside',{type:'geojson',data:{type:'Feature',properties:{},geometry:{type:'Polygon',coordinates:[[[-180,-85],[180,-85],[180,85],[-180,85],[-180,-85]],...rings]}}});
  const first=map.getStyle().layers.find(l=>l.type==='symbol')?.id;
  map.addLayer({id:'outside-fade',type:'fill',source:'outside',paint:{'fill-color':t.bg,'fill-opacity':.70}},first);
  map.addLayer({id:'ward-fill',type:'fill',source:'ward',paint:{'fill-color':BLUE,'fill-opacity':.035}},first);
  map.addLayer({id:'ward-border-halo',type:'line',source:'ward',paint:{'line-color':'#fff','line-width':8}});
  map.addLayer({id:'ward-border',type:'line',source:'ward',paint:{'line-color':'#183e56','line-width':3.6}});
 }else map.moveLayer('rail-context');
 map.addLayer({id:'station-names',type:'symbol',source:'openmaptiles','source-layer':'poi',filter:['any',['==',['get','class'],'railway'],['==',['get','subclass'],'station']],layout:{'text-field':['coalesce',['get','name_en'],['get','name']],'text-font':['Noto Sans Regular'],'text-size':ward?15:19,'text-offset':[0,1.15],'text-anchor':'top'},paint:{'text-color':t.ink,'text-halo-color':t.bg,'text-halo-width':2}});
 if(!ward)map.addLayer({id:'nearby-places',type:'symbol',source:'openmaptiles','source-layer':'poi',filter:['match',['get','class'],['catering','shop','food','restaurant','cafe'],true,false],layout:{'text-field':['coalesce',['get','name_en'],['get','name']],'text-font':['Noto Sans Regular'],'text-size':17,'text-variable-anchor':['top','bottom'],'text-radial-offset':.6,'text-max-width':9,'text-optional':true},paint:{'text-color':'#233943','text-halo-color':'#fff','text-halo-width':2}});
 await waitIdle(map);
 if(errors.length)throw Error(`Map errors: ${errors.slice(0,3).join('; ')}`);
 const canvas=document.createElement('canvas');canvas.width=canvas.height=2160;const ctx=canvas.getContext('2d');ctx.scale(2,2);
 ctx.fillStyle='#000';ctx.fillRect(0,0,1080,1080);ctx.fillStyle=BLUE;ctx.fillRect(0,0,1080,8);ctx.fillStyle=RED;for(let i=0;i<4;i++)star(ctx,874+i*47,44,16);
 label(ctx,ward?(permit?'THE WARD':'THE WARD SO FAR'):(permit?'AROUND THE PERMIT':'AROUND THE PREAPPROVAL'),42,55,21,BLUE);
 fitted(ctx,ward?`WARD ${snapshot.ward}`:snapshot.focus.address,42,133,995,ward?83:65);
 const rank=`${snapshot.tied?'TIED ':''}#${snapshot.rank} OF 50`;
 const detail=permit?(ward?`ADU BUILDING PERMIT #${snapshot.permit_number} ISSUED`:`ADU BUILDING PERMIT ISSUED   /   WARD ${snapshot.ward}`):(ward?`${snapshot.adus} ADUs  /  ${snapshot.applications} APPLICATIONS  /  ${rank}`:`${snapshot.focus.quantity} ADU${snapshot.focus.quantity===1?'':'s'} REQUESTED   /   WARD ${snapshot.ward}`);
 fitted(ctx,detail,43,171,993,24,BLUE,'Roboto');
 ctx.drawImage(map.getCanvas(),0,194,1080,height);
 if(ward&&!permit){
  for(const {f,p,x,y} of positions(map)){
   if(f.id===snapshot.focus.id)continue;
   if(Math.hypot(x-p.x,y-p.y)>4){ctx.strokeStyle='#354f60';ctx.lineWidth=1.5;ctx.beginPath();ctx.moveTo(p.x,p.y+194);ctx.lineTo(x,y+194);ctx.stroke();}
   dot(ctx,x,y+194,f.quantity,false);
  }
  const p=map.project(center);dot(ctx,p.x,p.y+194,snapshot.focus.quantity,true);
  ctx.strokeStyle='#b1c4ce';ctx.lineWidth=1;ctx.strokeRect(20,214,1040,height-40);
 }else{const p=map.project(center);pin(ctx,p.x,p.y+194);if(ward){ctx.strokeStyle='#b1c4ce';ctx.lineWidth=1;ctx.strokeRect(20,214,1040,height-40);}}
 const date=new Date(`${snapshot.as_of}T12:00:00Z`).toLocaleDateString('en-US',{month:'long',day:'numeric',year:'numeric',timeZone:'UTC'});
 if(ward)label(ctx,permit?`Building permit issued ${date} · Location is approximate`:`Applications submitted since April 1, 2026 · As of ${date}`,42,1027,19,'#cad1d5');
 ctx.textAlign='right';label(ctx,'Data: City of Chicago Data Portal · © OpenMapTiles · © OpenStreetMap contributors',1038,1061,12,'#c1c9cc');ctx.textAlign='start';ctx.fillStyle=BLUE;ctx.fillRect(0,1072,1080,8);
 let quality=.94,blob=await new Promise(resolve=>canvas.toBlob(resolve,'image/jpeg',quality));
 while(blob.size>1_950_000&&quality>.5){quality-=.04;blob=await new Promise(resolve=>canvas.toBlob(resolve,'image/jpeg',quality));}
 if(blob.size>2_000_000)throw Error('Scorecard JPEG exceeds Bluesky limit');
 const response=await fetch(`/export/${id}.jpg`,{method:'POST',body:blob});if(!response.ok)throw Error('Image export failed');
 results.push({id,bytes:blob.size,width:2160,height:2160,errors});
 map.remove();
}
await render('n5');await render('wc');
const response=await fetch('/export/verification.json',{method:'POST',body:JSON.stringify({mode:snapshot.mode??'scorecard',source_run:snapshot.source_run,ward:snapshot.ward,focus:snapshot.focus.id,renders:results})});
if(!response.ok)throw Error('Verification export failed');
document.querySelector('#status').textContent='Scorecard maps complete';
