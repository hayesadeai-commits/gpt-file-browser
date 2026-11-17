/**
 * Dependency-free regression harness: node tests/preflight.cjs.
 * Executes the production function/script text in isolated VM contexts with mock
 * fetch/DOM/image implementations. It never signs in or changes cloud files.
 * Rust format! escapes are unwrapped when extracting embedded browser scripts;
 * the mutation script is read from src/changes.js and assembled as changes.rs does;
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

// Execute the actual mutation script (src/changes.js) against a simulated service.
/** Assemble the script the way changes.rs does: preflight first, request data last. */
const changeScript=args=>source('changes.js').replace('__PREFLIGHT__',()=>source('preflight.js')).replace('__ARGS__',()=>JSON.stringify(args));
/**
 * A Library with a Trash listing of 50-item pages. Options describe how the service behaves:
 * whether Trash honours an ID or name filter, what a trashed file's own node returns, how
 * many Trash reads pass before a new entry becomes visible, and how many other files sit in
 * Trash ahead of it. `state` counts every kind of request so cost can be asserted.
 */
function libraryService(options={}){
 const opt={supportsId:false,supportsName:false,refuseId:false,nodeAfter:'missing',visibleAfter:0,deleteStatus:200,deleteWorks:true,others:0,staleStamp:false,startTrashed:false,...options};
 const state={mutations:0,node:0,by_id:0,by_name:0,walk:0};
 let trashed=opt.startTrashed,stamp=opt.startTrashed?'2026-11-01T00:00:00Z':null,trashReads=0;
 const others=Array.from({length:opt.others},(_,i)=>({id:'libfile_other_'+i,name:'other-'+i+'.png',trashed_at:'2026-01-01T00:00:00Z'}));
 const entry=()=>({...current,trashed_at:stamp});
 const reply=(status,body)=>({status,json:async()=>body});
 const fetch=async(url,request={})=>{
  const method=request.method||'GET';
  assert.equal(request.headers.Authorization,'test');
  if(method==='DELETE'){
   state.mutations++;assert.match(url,/soft_delete=true/,'Only a soft delete is ever requested');
   if(opt.deleteWorks){trashed=true;stamp=opt.staleStamp?'2020-01-01T00:00:00Z':new Date().toISOString();trashReads=0;}
   return reply(opt.deleteStatus,{});
  }
  if(url.endsWith('/restore')){assert.equal(method,'POST');state.mutations++;trashed=false;stamp=null;return reply(200,{});}
  if(method==='POST'){
   const q=JSON.parse(request.body);
   assert.equal(q.limit,50);assert.equal(q.trashed_only,true);assert.equal(q.include_hidden_files,true);
   const kind=q.allowlist_ids?'by_id':q.q!==undefined?'by_name':'walk';state[kind]++;
   const visible=trashed&&(opt.startTrashed||trashReads++>=opt.visibleAfter);
   const listing=visible?[...others,entry()]:others;
   const page=i=>({items:listing.slice(i*50,i*50+50),cursor:(i+1)*50<listing.length?String(i+1):null});
   if(kind==='by_id'){
    assert.deepEqual(q.allowlist_ids,[scanned.id]);
    if(opt.refuseId)return reply(422,{detail:'unknown field'});
    return reply(200,opt.supportsId?{items:visible?[entry()]:[],cursor:null}:page(0));
   }
   if(kind==='by_name'){
    assert.equal(q.q,scanned.name);
    return reply(200,opt.supportsName?{items:visible?[entry()]:[],cursor:null}:page(0));
   }
   return reply(200,page(q.cursor===null?0:Number(q.cursor)));
  }
  state.node++;
  if(!trashed)return reply(200,state.mutations&&opt.startTrashed?{...current,trashed_at:null}:current);
  return opt.nodeAfter==='trashed'?reply(200,entry()):reply(404,{});
 };
 return {fetch,state};
}
async function change(action,options,lookup){
 const service=libraryService(options);
 const script=changeScript({file:scanned,action,authorization:'test',lookup});
 const result=await vm.runInNewContext(script,{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,setTimeout:fn=>fn(),fetch:service.fetch});
 return {result,...service.state};
}
(async()=>{
 // Execute the actual split phases against one shared service. A sent receipt
 // releases request capacity, but only independent positive readback is success.
 for(const opt of [{supportsId:true},{supportsId:true,visibleAfter:Infinity},{supportsId:true,lostResponse:true}]){
  const service=libraryService(opt);
  const fetch=async(url,request)=>{
   const response=await service.fetch(url,request);
   if(opt.lostResponse&&request?.method==='DELETE')throw Error('response lost');
   return response;
  };
  const run=extra=>vm.runInNewContext(changeScript({file:scanned,action:'trash',authorization:'test',lookup:{by_id:true,node:false},...extra}),{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,setTimeout:fn=>fn(),fetch});
  const sent=await run({phase:'send'});
  assert.equal(sent.status,'sent');assert.equal(service.state.mutations,1);
  assert.equal(service.state.node,1);assert.equal(service.state.by_id+service.state.by_name+service.state.walk,0,'Sending must not wait for readback');
  if(opt.lostResponse)assert.equal(sent.mutation_status,null);
  const verified=await run({phase:'verify',sent:sent.sent,mutation_status:sent.mutation_status});
  assert.equal(verified.status,opt.visibleAfter===Infinity?'uncertain':'verified');
  assert.equal(service.state.mutations,1,'Verification must never repeat the mutation');
 }
 const hints=o=>JSON.parse(JSON.stringify(o));
 // A trashed file's node is gone and Trash is large. With an ID filter the file is
 // confirmed in one Trash request instead of one per 50 trashed files.
 let r=await change('trash',{supportsId:true,others:5000});
 assert.equal(r.result.status,'verified');assert.equal(r.mutations,1);
 assert.deepEqual([r.node,r.by_id,r.by_name,r.walk],[2,1,0,0],'First file: node read, then one filtered Trash request');
 assert.deepEqual(hints(r.result.lookup),{by_id:true,by_name:null,node:false});
 assert.equal(r.result.node.id,scanned.id);
 // What it learned makes the next file cost three requests however large Trash is.
 r=await change('trash',{supportsId:true,others:5000},{by_id:true,by_name:null,node:false});
 assert.equal(r.result.status,'verified');assert.equal(r.mutations,1);
 assert.deepEqual([r.node,r.by_id,r.by_name,r.walk],[1,1,0,0],'Later files: preflight, one mutation, one Trash request');
 // The listing can lag behind the request: a proven lookup is polled, not abandoned.
 r=await change('trash',{supportsId:true,others:5000,visibleAfter:2},{by_id:true,by_name:null,node:false});
 assert.equal(r.result.status,'verified');assert.deepEqual([r.by_id,r.walk,r.mutations],[3,0,1]);
 // A refused filter is dropped at once and the other one is used.
 r=await change('trash',{refuseId:true,supportsName:true,others:200});
 assert.equal(r.result.status,'verified');assert.deepEqual([r.by_id,r.by_name,r.walk,r.mutations],[1,1,0,1]);
 assert.deepEqual(hints(r.result.lookup),{by_id:false,by_name:true,node:false});
 // Filters the service ignores: the full listing still confirms the file (page 3 of Trash),
 // and both lookups are recorded as unusable...
 r=await change('trash',{others:120});
 assert.equal(r.result.status,'verified');assert.equal(r.mutations,1);assert.equal(r.walk,3);
 assert.deepEqual(hints(r.result.lookup),{by_id:false,by_name:false,node:false});
 assert.equal(r.by_id,1,'An ID filter that returns other files is recognised as ignored');
 // ...so later files send no filtered requests: only the node read and the full listing.
 r=await change('trash',{others:120},{by_id:false,by_name:false,node:false});
 assert.equal(r.result.status,'verified');
 assert.deepEqual([r.node,r.by_id,r.by_name,r.walk,r.mutations],[2,0,0,3,1],'No extra requests once a lookup is known not to work');
 // Delayed visibility with only the full listing available: readback is retried, the mutation is not.
 r=await change('trash',{visibleAfter:1},{by_id:false,by_name:false,node:false});
 assert.equal(r.result.status,'verified');assert.deepEqual([r.walk,r.mutations],[2,1]);
 // A quick result stamped before this request is not evidence that this request worked.
 r=await change('trash',{supportsId:true,staleStamp:true});
 assert.equal(r.result.status,'verified','The full listing remains the standard of proof');
 assert(r.walk>=1,'Stale quick evidence must fall through to the full listing');
 assert.equal(r.result.lookup.by_id,false);assert.equal(r.mutations,1);
 // Where the node itself shows the trashed state, no Trash request is needed at all.
 r=await change('trash',{nodeAfter:'trashed',others:5000});
 assert.equal(r.result.status,'verified');assert.deepEqual([r.node,r.by_id,r.by_name,r.walk,r.mutations],[2,0,0,0,1]);
 assert.equal(r.result.lookup.node,true);
 // Outcomes other than success are unchanged: a rejected request is a failure...
 r=await change('trash',{supportsId:true,supportsName:true,deleteWorks:false,deleteStatus:403});
 assert.equal(r.result.status,'failed');assert.match(r.result.message,/Service rejected the change \(HTTP 403\)/);assert.equal(r.mutations,1);
 // ...and a request whose effect cannot be seen anywhere stays uncertain, never retried.
 r=await change('trash',{supportsId:true,visibleAfter:Infinity});
 assert.equal(r.result.status,'uncertain');assert.match(r.result.message,/could not be verified \(HTTP 200\)/);assert.equal(r.mutations,1);
 // Hints cannot manufacture a success: with every shortcut claimed to work, nothing visible is still uncertain.
 r=await change('trash',{visibleAfter:Infinity},{by_id:true,by_name:true,node:true});
 assert.equal(r.result.status,'uncertain');assert.equal(r.mutations,1);
 // A file that cannot be read beforehand is a plain failure: nothing was sent.
 const offline=await vm.runInNewContext(changeScript({file:scanned,action:'trash',authorization:'test'}),{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,setTimeout:fn=>fn(),fetch:async()=>{throw Error('offline')}});
 assert.equal(offline.status,'failed');assert.match(offline.message,/Nothing was sent/);
 console.log('Trash verification checks passed: one-request lookup, learned hints, lag, refused/ignored filters, stale evidence, single mutation.');

 // Restore: the file is located in Trash with one request where a filter works.
 r=await change('restore',{supportsId:true,startTrashed:true,others:5000});
 assert.equal(r.result.status,'verified');assert.deepEqual([r.by_id,r.walk,r.mutations],[1,0,1]);
 // Without a usable filter, restore still reads Trash to the end; failures there send nothing.
 const restoreScript=changeScript({file:scanned,action:'restore',authorization:'test'});
 for(const mode of ['large','missing-cursor','repeated-cursor','exhausted']){
  let reads=0,mutations=0;
  const result=await vm.runInNewContext(restoreScript,{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,setTimeout:fn=>fn(),fetch:async(url,options)=>{
   let body,status=200;
   if(url.endsWith('/restore')){mutations++;body={};}
   else if(options.method==='POST'){
    const q=JSON.parse(options.body);assert.equal(q.limit,50);
    // This service ignores the ID and name filters and answers with an ordinary page.
    if(q.allowlist_ids||q.q!==undefined)body={items:Array.from({length:50},(_,i)=>({id:'libfile_unrelated_'+i})),cursor:'more'};
    else {
     reads++;const matched=mode==='large'&&reads===101;
     body={items:matched?[{...current,trashed_at:'2026-11-14T21:43:00Z'}]:Array.from({length:50},(_,i)=>({id:'libfile_other_'+reads+'_'+i})),cursor:matched?null:(mode==='repeated-cursor'?'same':String(reads))};
     if(mode==='missing-cursor')delete body.cursor;
    }
   }else body={...current,trashed_at:null};
   return {status,json:async()=>body};
  }});
  if(mode==='large'){assert.equal(result.status,'verified');assert.equal(reads,101);assert.equal(mutations,1);assert.deepEqual(hints(result.lookup),{by_id:false,by_name:false,node:null});}
  else {assert.equal(result.status,'failed');assert.equal(mutations,0);if(mode==='exhausted'){assert.equal(reads,1000);assert.match(result.message,/too large to check/);}}
 }
 console.log('Restore checks passed: one-request lookup, large Trash, malformed/repeated/exhausted pagination.');
})().catch(e=>{console.error(e);process.exitCode=1;});

// Execute the actual listing fetch: bounded retry, slim rows, and preserved validation shape.
(async()=>{
 const template=embedded('service.rs');
 const fields=['id','name','file_size_bytes','trashed_at'];
 const run=async(replies,args={})=>{
  let calls=0;const waits=[];
  const script=template.replace('{args}',JSON.stringify({url:'https://chatgpt.com/backend-api/files/library/nodes',authorization:'test',account:'acct',fields,...args}));
  const result=await vm.runInNewContext(script,{location:{origin:'https://chatgpt.com'},AbortController,clearTimeout,setTimeout:(fn,ms)=>{if(ms<30000){waits.push(ms);fn();}return 0;},fetch:async(url,options)=>{
   assert.equal(options.method,args.body?'POST':'GET');if(args.body){assert.deepEqual(JSON.parse(options.body),args.body);assert.equal(options.headers['Content-Type'],'application/json');}assert.equal(options.headers.Authorization,'test');assert.equal(options.headers['ChatGPT-Account-ID'],'acct');
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
 const postBody={limit:50,cursor:'next',include_hidden_files:true,trashed_only:false};
 r=await run([{status:503},{status:200,body:page}],{url:'https://chatgpt.com/backend-api/files/library',body:postBody});
 assert.equal(r.calls,2);assert.equal(r.result.status,200);assert.equal(r.result.body.items[0].id,'libfile_a');
 console.log('Listing fetch checks passed: GET/POST, exact read-only body, bounded retry, slim rows, validation shape.');
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

// Controlled-latency benchmark of production per-file scripts, with no network access.
// Rust tests enforce the scheduler cap; this checks that overlapping scripts retain
// their independent preflight, single mutation, and positive readback sequence.
(async()=>{
 async function benchmark(concurrency){
  const files=Array.from({length:8},(_,i)=>({...scanned,id:'libfile_parallel_'+i,file_id:'file_fixture_'+i,name:'fixture-'+i+'.txt'}));
  const states=new Map(files.map(f=>[f.id,{file:f,events:[],trashed:false}]));
  let next=0,active=0,peak=0;
  const fetch=async(url,req={})=>{
   await new Promise(resolve=>setTimeout(resolve,20));
   const method=req.method||'GET';
   const id=method==='POST'?JSON.parse(req.body).allowlist_ids[0]:decodeURIComponent(url.match(/\/files\/(libfile_[^/?]+)/)[1]);
   const state=states.get(id);assert(state);const f=state.file;
   const node={...current,id:f.id,file_id:f.file_id,name:f.name,trashed_at:state.trashed?new Date().toISOString():null};
   if(method==='DELETE'){
    assert.deepEqual(state.events,['preflight']);state.events.push('delete');state.trashed=true;
    return {status:200,json:async()=>({})};
   }
   if(method==='POST'){
    assert(state.trashed);state.events.push('verify');
    return {status:200,json:async()=>({items:[node],cursor:null})};
   }
   state.events.push('preflight');return {status:200,json:async()=>node};
  };
  const start=performance.now();
  await Promise.all(Array.from({length:concurrency},async()=>{
   while(next<files.length){
    const file=files[next++];active++;peak=Math.max(peak,active);
    const result=await vm.runInNewContext(changeScript({file,action:'trash',authorization:'test',lookup:{by_id:true,by_name:false,node:false}}),{location:{origin:'https://chatgpt.com'},URLSearchParams,AbortSignal,Date,setTimeout,fetch});
    active--;assert.equal(result.status,'verified');assert.equal(result.node.id,file.id);
   }
  }));
  for(const state of states.values())assert.deepEqual(state.events,['preflight','delete','verify']);
  assert(peak<=concurrency);return {ms:Math.round(performance.now()-start),peak};
 }
 const serial=await benchmark(1),parallel=await benchmark(4);assert.equal(parallel.peak,4);
 console.log(`Checked deletion pipeline (mock network): serial ${serial.ms} ms, four in flight ${parallel.ms} ms; every file preflighted, sent once, and verified.`);
})().catch(e=>{console.error(e);process.exitCode=1;});

// Progress must not turn a sent request into success or release its reservation.
{
 const nodes=new Map();
 const element=()=>({style:{},append(){},replaceChildren(){},remove(){}});
 const $=id=>{if(!nodes.has(id))nodes.set(id,element());return nodes.get(id)};
 const reservedIds=new Set(['libfile_fixture']);
 const context=vm.createContext({window:{},document:{createDocumentFragment:element,createElement:element,getElementById:()=>null},$,reservedIds,reservedVersion:0,jobActive:false,queuedBatchCount:0,queuedFileCount:0,requestRender(){vm.runInContext('paintJob()',context)},updateActions(){}});
 vm.runInContext(region('inventory.js','batch_progress'),context);
 const data={id:1,action:'trash',active:true,offset:0,verified:0,attention:0,skipped:0,remaining:1,requests_sent:1,awaiting_confirmation:1,outcomes:[],confirmation_rows:[{id:'libfile_fixture',name:'fixture.png',verifying:true}]};
 context.window.showJob(data);
 assert(reservedIds.has('libfile_fixture'));
 assert.match($('job-text').textContent,/1 requests sent.*0 confirmed in Trash/);
 assert.equal($('job-bar').style.width,'0%');assert.equal($('job-toggle').hidden,false);
 context.window.showJob({...data,active:false,verified:1,remaining:0,awaiting_confirmation:0,confirmation_rows:[],outcomes:[{id:'libfile_fixture',name:'fixture.png',status:'verified',message:'Verified'}]});
 assert(!reservedIds.has('libfile_fixture'));assert.equal($('job-bar').style.width,'100%');
 console.log('Split progress checks passed: sent is pending, verified releases reservations.');
}
