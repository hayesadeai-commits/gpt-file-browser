/**
 * Dependency-free regression harness: node tests/preflight.cjs.
 * Executes the production function/script text in isolated VM contexts with mock
 * fetch/DOM/image implementations. It never signs in or changes cloud files.
 * Rust format! escapes are unwrapped when extracting embedded browser scripts;
 * UI logic is taken from the "#region" blocks in src/inventory.js and src/thumbnails.js.
 * Update these extraction points if that code moves.
 * Mock image tests cover fallback/control flow, not native WebView codec behavior.
 */
const assert=require('node:assert/strict');
const fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const source=name=>fs.readFileSync(path.join(__dirname,'../src',name),'utf8');
/** Production text between "// #region name" and "// #endregion name". */
function region(file,name){
 const text=source(file),start=text.indexOf('// #region '+name+'\n'),end=text.indexOf('// #endregion '+name);
 assert(start>=0&&end>start,`Missing region ${name} in ${file}`);
 return text.slice(start,end);
}
/** The first raw string in a Rust file, with format! brace escapes removed. */
const embedded=file=>source(file).match(/r#"([\s\S]*?)"#/)[1].replaceAll('{{','{').replaceAll('}}','}');

const check=vm.runInNewContext('('+source('preflight.js')+')');
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
const script=embedded('changes.rs').replace('{args}',JSON.stringify({file:scanned,action:'trash',authorization:'test'})).replace('{preflight}',source('preflight.js'));
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

// Execute the actual listing fetch: bounded retry, slim rows, and preserved validation shape.
(async()=>{
 const template=embedded('service.rs');
 const fields=['id','name','file_size_bytes','trashed_at'];
 const run=async(replies,args={})=>{
  let calls=0;const waits=[];
  const script=template.replace('{args}',JSON.stringify({url:'https://chatgpt.com/backend-api/files/library/nodes',authorization:'test',account:'acct',fields,...args}));
  const result=await vm.runInNewContext(script,{location:{origin:'https://chatgpt.com'},AbortController,clearTimeout,setTimeout:(fn,ms)=>{if(ms<30000){waits.push(ms);fn();}return 0;},fetch:async(url,options)=>{
   assert.equal(options.method,'GET');assert.equal(options.headers.Authorization,'test');assert.equal(options.headers['ChatGPT-Account-ID'],'acct');
   const reply=replies[Math.min(calls++,replies.length-1)];
   return {status:reply.status,headers:{get:name=>name==='retry-after'?reply.retryAfter??null:null},body:{cancel:async()=>{}},text:async()=>reply.text??JSON.stringify(reply.body)};
  }});
  return {result,calls,waits};
 };
 const page={items:[{id:'libfile_a',name:'a.png',file_size_bytes:5,trashed_at:null,thumbnail:{url:'private'},owner:{email:'x'}},null],cursor:'next',site_projects:[1,2]};
 // Throttling is retried with backoff, then the page is reduced to the listed fields only.
 let r=await run([{status:429,retryAfter:'2'},{status:503},{status:200,body:page}]);
 assert.equal(r.calls,3);assert.deepEqual(r.waits,[2000,1000],'Retry-After is honoured, otherwise exponential backoff');
 assert.equal(r.result.status,200);
 assert.deepEqual(JSON.parse(JSON.stringify(r.result.body)),{items:[{id:'libfile_a',name:'a.png',file_size_bytes:5,trashed_at:null},null],cursor:'next'});
 // A persistent gateway error is reported after three retries rather than looping.
 r=await run([{status:502,body:{detail:'bad gateway'}}]);
 assert.equal(r.calls,4);assert.equal(r.result.status,502);
 // Other statuses are never retried, and a non-JSON error page still reports its status.
 r=await run([{status:403,text:'<html>challenge</html>'}]);
 assert.equal(r.calls,1);assert.equal(r.result.status,403);assert.equal(r.result.body,null);
 r=await run([{status:422,body:{detail:'limit too large'}}]);
 assert.equal(r.calls,1);assert.equal(r.result.status,422);
 // The slim copy must not invent pagination evidence the service did not send.
 r=await run([{status:200,body:{items:[]}}]);
 assert.equal(JSON.parse(JSON.stringify(r.result.body)).cursor,undefined,'A missing cursor stays missing');
 r=await run([{status:200,body:{items:'unexpected',cursor:null}}]);
 assert.equal(r.result.body.items,'unexpected','Unexpected shapes pass through for native validation');
 // Usage replies are not listings and are returned unchanged.
 r=await run([{status:200,body:{used_bytes:1,allowed_bytes:2}}],{fields:null});
 assert.deepEqual(JSON.parse(JSON.stringify(r.result.body)),{used_bytes:1,allowed_bytes:2});
 await assert.rejects(run([{status:200,text:'x'.repeat(2097153)}]),/too large/);
 await assert.rejects(vm.runInNewContext(template.replace('{args}','{}'),{location:{origin:'https://example.test'}}),/Sign-in required/);
 console.log('Listing fetch checks passed: bounded retry, backoff, slim rows, status passthrough, size limit.');
})().catch(e=>{console.error(e);process.exitCode=1;});

// Hover lifecycle: opening, leaving during fetch, and cached re-entry.
{
 const handlers={},elements=[],sent=[],before=[];let timer;
 function element(){return {style:{},dataset:{},hidden:false,children:[],setAttribute(){},append(...v){this.children.push(...v)},replaceChildren(...v){this.children=v},getBoundingClientRect(){return {left:370,right:402,top:360,width:500,height:400}}};}
 const context=vm.createContext({document:{body:{append(){}},createElement(){const el=element();elements.push(el);return el;},addEventListener(n,f){handlers[n]=f}},window:{addEventListener(){}},innerWidth:1280,innerHeight:720,previewEpoch:0,previewKey:r=>JSON.stringify([r.id,r.modified||'']),send:v=>sent.push(v),renderHooks:{before},setTimeout:fn=>{timer=fn;return 1},clearTimeout:()=>{timer=null}});
 vm.runInContext(region('thumbnails.js','hover'),context);
 const box={...element(),isConnected:true,dataset:{id:'libfile_hover',name:'icon.png',modified:'',key:'["libfile_hover",""]'},closest(){return this},contains(other){return other===this}};
 handlers.pointerover({target:box});timer();assert.equal(JSON.parse(sent[0].slice(10)).full,true);
 const panel=elements[0];assert.equal(panel.id,'hover-preview');assert.equal(panel.hidden,false);
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
 // Retries are bounded: a load that keeps failing is eventually shown as unavailable.
 handlers.pointerover({target:box});timer();
 for(let i=0;i<10;i++){vm.runInContext('showHoverPreview({epoch:0,id:"libfile_hover",modified:null,data:null,retry:true})',context);timer();}
 vm.runInContext('showHoverPreview({epoch:0,id:"libfile_hover",modified:null,data:null,retry:true})',context);
 assert.equal(vm.runInContext('hoverCache.get(\'["libfile_hover",""]\')',context),null,'Retries stop after the limit');
 // The table announces a rebuild through its hook, which must dismiss the overlay.
 handlers.pointerover({target:box});timer();assert.equal(panel.hidden,false);
 assert.equal(before.length,1);before[0]();assert.equal(panel.hidden,true);

 console.log('Hover lifecycle checks passed: load, pointer leave, late response, cache, scroll dismissal, bounded retry.');
}

// Range selection follows the full sorted/filtered collection, across pages.
{
 const selected=new Set();const context=vm.createContext({selected});vm.runInContext(region('inventory.js','selection'),context);
 context.files=Array.from({length:110},(_,i)=>({id:String(i),action_blocked:i===50}));
 vm.runInContext('selectRange(files,"1",true,false);selectRange(files,"105",true,true)',context);
 assert.equal(selected.size,104);assert(!selected.has('50'));assert(selected.has('105'));
 vm.runInContext('selectRange(files,"100",false,true)',context);assert(!selected.has('100'));assert(selected.has('105'));
 vm.runInContext('selectRange(files.slice(106),"109",true,true)',context);assert(selected.has('109'));
 console.log('Shift-click checks passed: range, cross-page, deselection, blocked files, missing anchor.');
}

// Execute the actual image loader: a failed thumbnail must fall back to JPG content.
(async()=>{
 const template=embedded('preview.rs');
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
// The same context then exercises whole-collection filtering, sorting, and its cache.
{
 const selected=new Set(['a','b','next']);let rendered=0;
 const inputs={findfiles:'',typefilter:'all',folderfilter:''};
 const context=vm.createContext({window:{},selected,renderFiles(){rendered++},$:id=>({value:inputs[id]})});
 vm.runInContext('let selectionAnchor="a";'+region('inventory.js','queue'),context);
 context.window.showQueue({ids:['a','b'],batches:1,files:2});
 assert.deepEqual([...selected],['next']);assert.equal(vm.runInContext('selectionAnchor',context),null);
 assert.equal(vm.runInContext('reservedIds.has("a")',context),true);
 context.window.showQueue({ids:[],batches:0,files:0});
 assert.equal(vm.runInContext('reservedIds.size',context),0);assert.deepEqual([...selected],['next']);assert.equal(rendered,2);

 vm.runInContext(region('inventory.js','format'),context);
 const row=(id,name,extra={})=>({id,name,mime:'',parent:'',folder:false,bytes:null,modified:null,...extra});
 vm.runInContext('let viewMode="files",sortKey="name",sortDirection=1;const stores={files:{rows:new Map(),version:0},trash:{rows:new Map(),version:0}};function load(view,rows){stores[view].rows=new Map(rows.map(r=>[r.id,prepare(r)]));stores[view].version++}'+region('inventory.js','filter'),context);
 const ids=()=>vm.runInContext('filteredFiles().map(f=>f.id).join(",")',context);
 context.rows=[row('a','A'),row('next','Next')];vm.runInContext('load("files",rows)',context);
 context.window.showQueue({ids:['a'],batches:1,files:1});
 assert.equal(ids(),'next');
 context.window.showQueue({ids:[],batches:0,files:0});
 assert.equal(ids(),'a,next');

 // Natural name order, numeric size order, parsed dates; unknown values always sort last.
 context.rows=[row('f10','file10.txt',{bytes:5,modified:'2026-03-01T00:00:00Z'}),row('f2','File2.txt',{bytes:900,modified:'2026-01-01T00:00:00+00:00'}),row('none','zeta.bin'),row('dir','Folder',{folder:true})];
 vm.runInContext('load("files",rows)',context);
 assert.equal(ids(),'f2,f10,none','Folders are not listed; names sort naturally');
 vm.runInContext('sortKey="bytes";sortDirection=-1',context);assert.equal(ids(),'f2,f10,none');
 vm.runInContext('sortDirection=1',context);assert.equal(ids(),'f10,f2,none','Unknown size stays last in both directions');
 vm.runInContext('sortKey="modified";sortDirection=-1',context);assert.equal(ids(),'f10,f2,none');
 // The result is reused until something it depends on changes.
 assert.equal(vm.runInContext('filteredFiles()===filteredFiles()',context),true,'Unchanged inputs reuse the sorted result');
 context.before=vm.runInContext('filteredFiles()',context);
 inputs.findfiles='FILE2';assert.equal(ids(),'f2','Search is case-insensitive');
 inputs.findfiles='';inputs.typefilter='document';assert.equal(ids(),'f10,f2');
 inputs.typefilter='all';
 assert.equal(vm.runInContext('filteredFiles()===before',context),false,'Changed filters are recomputed, not served stale');
 // Folder filters include descendants and tolerate cyclic parent links.
 context.rows=[row('d1','Top',{folder:true,parent:'d3'}),row('d2','Inner',{folder:true,parent:'d1'}),row('d3','Loop',{folder:true,parent:'d2'}),row('x','x.txt',{parent:'d2'}),row('y','y.txt',{parent:'dz'}),row('z','z.txt',{parent:''})];
 vm.runInContext('load("files",rows);sortKey="name";sortDirection=1',context);
 inputs.folderfilter='d1';assert.equal(ids(),'x');
 inputs.folderfilter='d2';assert.equal(ids(),'x');
 inputs.folderfilter='';assert.equal(ids(),'x,y,z');
 // Trash is a separate store and ignores the Files folder filter.
 context.rows=[row('t','trashed.txt',{parent:'gone'})];inputs.folderfilter='d1';
 vm.runInContext('load("trash",rows);viewMode="trash"',context);assert.equal(ids(),'t');
 console.log('Queue UI and filter checks passed: unselection, hidden rows, sorting, search, folders, cache reuse.');

 const text=n=>vm.runInContext(`bytesText(${n})`,context);
 assert.equal(text(null),'Unknown');assert.equal(text(0),'0 B');assert.equal(text(999),'999 B');
 assert.equal(text(1536),(1.5).toLocaleString()+' KB');assert.equal(text(461.15*2**20),'461 MB');
 assert.equal(text(96.34*2**30),(96.3).toLocaleString()+' GB');assert.equal(text(1016*2**20),(0.99).toLocaleString()+' GB','No four-digit values');
 const kind=(name,mime='')=>vm.runInContext(`fileKind(${JSON.stringify({name,mime})})`,context);
 assert.equal(kind('a.CSV'),'sheet');assert.equal(kind('clip.mp4'),'video');assert.equal(kind('x','audio/mpeg'),'audio');
 assert.equal(kind('photo','image/png'),'image');assert.equal(kind('b.tar.gz'),'archive');assert.equal(kind('notes.md'),'document');assert.equal(kind('blob.bin'),'other');
 console.log('Formatting checks passed: byte sizes and file kinds.');
}
