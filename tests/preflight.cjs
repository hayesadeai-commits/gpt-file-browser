/**
 * Dependency-free regression harness: node tests/preflight.cjs.
 * Executes the production function/script text in isolated VM contexts with mock
 * fetch/DOM/image implementations. It never signs in or changes cloud files.
 * Rust format! escapes are unwrapped when extracting embedded browser scripts;
 * update these extraction points if those scripts move to another source file.
 * Mock image tests cover fallback/control flow, not native WebView codec behavior.
 */
const assert=require('node:assert/strict');
const fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const check=vm.runInNewContext('('+fs.readFileSync(path.join(__dirname,'../src/preflight.js'),'utf8')+')');
const scanned={id:'libfile_legacy',file_id:'file_original',name:'icon.png',bytes:271828,parent:'libdir_root',modified:'2026-01-15T08:30:45.123456Z'};
const current={id:scanned.id,file_id:scanned.file_id,name:scanned.name,file_size_bytes:scanned.bytes,parent_directory_id:scanned.parent,access_kind:'owned',updated_at:null};
assert.equal(check(scanned,current),null,'Legacy null timestamp must not block an unchanged file');
assert.equal(check(scanned,{...current,updated_at:'2026-01-15T08:30:45.123456+00:00'}),null,'Equivalent timestamp formats');
assert.equal(check(scanned,{...current,updated_at:'2026-01-16T08:30:45Z'}),'modified time');
assert.equal(check(scanned,{...current,file_size_bytes:999}),'size');
assert.equal(check(scanned,{...current,name:'renamed.png'}),'name');
assert.equal(check(scanned,{...current,parent_directory_id:'libdir_other'}),'folder');
assert.equal(check(scanned,{...current,file_id:'file_new_version'}),'file version');
assert.equal(check(scanned,{...current,access_kind:'shared'}),'ownership');
assert.equal(check(scanned,{...current,id:'libfile_different'}),'identity');
console.log('9 preflight regression checks passed.');

// Execute the actual embedded mutation script against a simulated service.
(async()=>{
const source=fs.readFileSync(path.join(__dirname,'../src/changes.rs'),'utf8');
const template=source.match(/r#"([\s\S]*?)"#/)[1];
const script=template.replaceAll('{{','{').replaceAll('}}','}').replace('{args}',JSON.stringify({file:scanned,action:'trash',authorization:'test'})).replace('{preflight}',fs.readFileSync(path.join(__dirname,'../src/preflight.js'),'utf8'));
for(const delayed of [false,true]){
 let mutations=0,reads=0;
 const result=await vm.runInNewContext(script,{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,setTimeout:fn=>fn(),fetch:async(url,options)=>{
  let status=200,body;
  if(options.method==='DELETE'){mutations++;body={};}
  else if(options.method==='POST'){assert.equal(JSON.parse(options.body).limit,50);reads++;body={items:delayed&&reads===1?[]:[{...current,trashed_at:'2026-11-14T21:43:00Z'}],cursor:null};}
  else if(mutations){status=404;body={};}else body=current;
  return {status,json:async()=>body};
 }});
 assert.equal(result.status,'verified');assert.equal(mutations,1,'Never repeat the mutation');assert.equal(reads,delayed?2:1);
}
const restoreScript=script.replace('"action":"trash"','"action":"restore"');
for(const mode of ['large','missing-cursor','repeated-cursor','exhausted']){
 let reads=0,mutations=0;
 const result=await vm.runInNewContext(restoreScript,{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,setTimeout:fn=>fn(),fetch:async(url,options)=>{
  let body,status=200;
  if(url.endsWith('/restore')){mutations++;body={};}
  else if(options.method==='POST'){
   reads++;const matched=mode==='large'&&reads===101;
   body={items:matched?[{...current,trashed_at:'2026-11-14T21:43:00Z'}]:Array.from({length:50},(_,i)=>({id:'libfile_other_'+reads+'_'+i})),cursor:matched?null:(mode==='repeated-cursor'?'same':String(reads))};
   if(mode==='missing-cursor')delete body.cursor;
  }else body={...current,trashed_at:null};
  return {status,json:async()=>body};
 }});
 if(mode==='large'){assert.equal(result.status,'verified');assert.equal(reads,101);assert.equal(mutations,1);}
 else {assert.equal(result.status,'failed');assert.equal(mutations,0);if(mode==='exhausted'){assert.equal(reads,1000);assert.match(result.message,/exceeded 1,000/);}}
}
console.log('Large Trash restore and malformed/repeated/exhausted pagination checks passed.');
console.log('Trash verification checks passed: missing node, supported page size, delayed visibility, single mutation.');
})().catch(e=>{console.error(e);process.exitCode=1;});

// Hover lifecycle: opening, leaving during fetch, and cached re-entry.
{
 const handlers={},elements=[],sent=[];let timer;
 function element(){return {style:{},dataset:{},hidden:false,children:[],setAttribute(){},append(...v){this.children.push(...v)},replaceChildren(...v){this.children=v},getBoundingClientRect(){return {left:370,right:402,top:360,width:500,height:400}}};}
 const context=vm.createContext({document:{head:{append(){}},body:{append(){}},createElement(){const el=element();elements.push(el);return el;},addEventListener(n,f){handlers[n]=f}},window:{addEventListener(){}},innerWidth:1280,innerHeight:720,previewEpoch:0,previewKey:r=>JSON.stringify([r.id,r.modified||'']),send:v=>sent.push(v),renderFiles(){},setTimeout:fn=>{timer=fn;return 1},clearTimeout:()=>{timer=null}});
 const code=fs.readFileSync(path.join(__dirname,'../src/thumbnails.js'),'utf8').split('// Hover stays in the file table:')[1];
 vm.runInContext('// Hover stays in the file table:'+code,context);
 const box={...element(),isConnected:true,dataset:{id:'libfile_hover',name:'icon.png',modified:'',key:'["libfile_hover",""]'},closest(){return this},contains(other){return other===this}};
 handlers.pointerover({target:box});timer();assert.equal(JSON.parse(sent[0].slice(10)).full,true);
 const panel=elements[1];assert.equal(panel.hidden,false);
 handlers.pointerout({target:box,relatedTarget:null});assert.equal(panel.hidden,true);
 vm.runInContext('showHoverPreview({epoch:0,id:"libfile_hover",modified:null,data:"data:image/png;base64,test"})',context);
 assert.equal(panel.hidden,true,'Late image response must not reopen dismissed popup');
 handlers.pointerover({target:box});timer();assert.equal(panel.hidden,false);assert.equal(sent.length,1,'Cache avoids a repeated image request');
 assert.equal(panel.children[0].src,'data:image/png;base64,test');
 handlers.scroll();assert.equal(panel.hidden,true);
 vm.runInContext('hoverCache.clear()',context);
 handlers.pointerover({target:box});timer();const beforeBusy=sent.length;
 vm.runInContext('showHoverPreview({epoch:0,id:"libfile_hover",modified:null,data:null,retry:true})',context);
 assert.equal(vm.runInContext('hoverCache.size',context),0,'Busy response must not be cached as unavailable');
 timer();assert.equal(sent.length,beforeBusy+1,'A visible hover retries after capacity is released');
 handlers.pointerout({target:box,relatedTarget:null});
 vm.runInContext('showHoverPreview({epoch:0,id:"libfile_hover",modified:null,data:null,retry:true})',context);
 const beforeLeave=sent.length;timer();assert.equal(sent.length,beforeLeave,'No retry after leaving the thumbnail');

 console.log('Hover lifecycle checks passed: load, pointer leave, late response, cache, scroll dismissal.');
}

// Range selection follows the full sorted/filtered collection, across pages.
{
 const source=fs.readFileSync(path.join(__dirname,'../src/inventory.js'),'utf8');
 const snippet=source.slice(source.indexOf('let selectionAnchor='),source.indexOf('function bytesText'));
 const selected=new Set();const context=vm.createContext({selected});vm.runInContext(snippet,context);
 context.files=Array.from({length:110},(_,i)=>({id:String(i),action_blocked:i===50}));
 vm.runInContext('selectRange(files,"1",true,false);selectRange(files,"105",true,true)',context);
 assert.equal(selected.size,104);assert(!selected.has('50'));assert(selected.has('105'));
 vm.runInContext('selectRange(files,"100",false,true)',context);assert(!selected.has('100'));assert(selected.has('105'));
 vm.runInContext('selectRange(files.slice(106),"109",true,true)',context);assert(selected.has('109'));
 console.log('Shift-click checks passed: range, cross-page, deselection, blocked files, missing anchor.');
}

// Execute the actual image loader: a failed thumbnail must fall back to JPG content.
(async()=>{
 const source=fs.readFileSync(path.join(__dirname,'../src/preview.rs'),'utf8');
 const template=source.match(/r#"([\s\S]*?)"#/)[1].replaceAll('{{','{').replaceAll('}}','}');
 for(const broken of ['http','network','decode']){
  const calls=[];let decoded=0;
  class ImageStub {naturalWidth=640;naturalHeight=480;set src(v){queueMicrotask(()=>{decoded++;if(broken==='decode'&&decoded===1)this.onerror();else this.onload();});}}
  class URLStub extends URL {static createObjectURL(){return 'blob:test'}static revokeObjectURL(){}}
  const script=template.replace('{args}',JSON.stringify({id:'libfile_jpg',name:'photo.JPG',mime:'application/octet-stream',bytes:5000000,full:false,authorization:'test'}));
  const data=await vm.runInNewContext(script,{location:{origin:'https://chatgpt.com'},URL:URLStub,Blob,AbortSignal,Image:ImageStub,setTimeout,clearTimeout,document:{createElement(){return {getContext(){return {drawImage(){}}},toDataURL(){return 'data:image/png;base64,decoded'}}}},fetch:async(url)=>{
   calls.push(url);
   if(url.includes('/thumbnail_url'))return {ok:true,json:async()=>({thumbnail_url:'https://chatgpt.com/thumb.jpg'})};
   if(url.includes('/content_url'))return {ok:true,json:async()=>({content_url:'https://chatgpt.com/original.jpg'})};
   if(url.endsWith('thumb.jpg')){if(broken==='network')throw Error('network');if(broken==='http')return {ok:false,headers:{get(){return null}}};}
   let read=false;return {ok:true,headers:{get(k){return k==='content-type'?'image/jpeg':null}},body:{getReader(){return {read:async()=>read?{done:true}:(read=true,{done:false,value:new Uint8Array([255,216,255])}),cancel:async()=>{}}}}};
  }});
  assert.equal(data,'data:image/png;base64,decoded');assert(calls.some(c=>c.endsWith('original.jpg')));
 }
 console.log('JPG fallback checks passed: HTTP failure, network error, decode failure, larger original.');
})().catch(e=>{console.error(e);process.exitCode=1;});

// Queue updates hide confirmed IDs and remove selection immediately, without dropping rows.
{
 const source=fs.readFileSync(path.join(__dirname,'../src/inventory.js'),'utf8');
 const start=source.indexOf('let reservedIds=');const end=source.indexOf('let selectionAnchor=',start);
 const selected=new Set(['a','b','next']);let rendered=0;
 const context=vm.createContext({window:{},selected,renderFiles(){rendered++}});
 vm.runInContext('let selectionAnchor="a";'+source.slice(start,end),context);
 context.window.showQueue({ids:['a','b'],batches:1,files:2});
 assert.deepEqual([...selected],['next']);assert.equal(vm.runInContext('selectionAnchor',context),null);
 assert.equal(vm.runInContext('reservedIds.has("a")',context),true);
 context.window.showQueue({ids:[],batches:0,files:0});
 assert.equal(vm.runInContext('reservedIds.size',context),0);assert.deepEqual([...selected],['next']);assert.equal(rendered,2);
 const filterCode=source.slice(source.indexOf('function filteredFiles()'),source.indexOf('function renderFiles()'));
 Object.assign(context,{document:{getElementById(id){return {value:id==='typefilter'?'all':''}}},inventoryState:{rows:[{id:'a',name:'A',folder:false},{id:'next',name:'Next',folder:false}]},sortKey:'name',sortDirection:1});
 vm.runInContext(filterCode,context);
 context.window.showQueue({ids:['a'],batches:1,files:1});
 assert.equal(vm.runInContext('filteredFiles().map(f=>f.id).join(",")',context),'next');
 context.window.showQueue({ids:[],batches:0,files:0});
 assert.equal(vm.runInContext('filteredFiles().length',context),2);
 console.log('Queue UI checks passed: immediate unselection, anchor reset, hidden rows, reservation release.');
}
