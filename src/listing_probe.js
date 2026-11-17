/** Read-only endpoint comparison. Only aggregate evidence leaves the WebView.
 * File names, IDs, session values, and listing bodies stay in this invocation's memory.
 * The fixed POST route below is a listing operation, never a mutation endpoint.
 */
(async()=>{
  if(location.origin!=='https://chatgpt.com')throw Error('Open ChatGPT first');
  const a=__PROBE_ARGS__,headers={Authorization:a.authorization};
  if(a.account)headers['ChatGPT-Account-ID']=a.account;
  async function request(url,body){
    for(let attempt=0;;attempt++){
      const response=await fetch(url,{method:body?'POST':'GET',credentials:'include',headers:body?{...headers,'Content-Type':'application/json'}:headers,body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(30000)});
      if([429,502,503,504].includes(response.status)&&attempt<3){await response.body?.cancel();await new Promise(r=>setTimeout(r,500*2**attempt));continue;}
      const text=await response.text();if(text.length>2097152)throw Error('Listing too large');
      let value=null;try{value=JSON.parse(text)}catch{}
      return {status:response.status,value};
    }
  }
  function page(r){if(r.status!==200)throw Error('Listing HTTP '+r.status);const b=r.value;if(!Array.isArray(b?.items)||!Object.hasOwn(b,'cursor')||!(b.cursor===null||typeof b.cursor==='string'))throw Error('Invalid listing page');return b;}
  function row(f){return {id:f.id,name:f.name??f.file_name,bytes:f.file_size_bytes??null,modified:f.updated_at??null,mime:f.mime_type??'',parent:f.parent_directory_id??f.directory_id??'',file_id:f.file_id??null,access:f.access_kind??'',artifact:f.library_artifact_type??''};}
  function add(result,items,folderOnly=false){for(const f of items){if(f.trashed_at!=null)continue;if(typeof f.id!=='string')throw Error('Missing identity');if(f.id.startsWith('libfile_')){if(folderOnly){result.unexpected_files++;continue;}if(result.files.has(f.id))result.duplicates++;result.files.set(f.id,row(f));}else if(f.id.startsWith('libdir_'))result.folders.set(f.id,row(f));}}
  function empty(){return {files:new Map(),folders:new Map(),pages:0,duplicates:0,unexpected_files:0,ms:0};}
  async function nodes(folderOnly){
    const result=empty(),queue=[{parent:null,cursor:null}],seen=new Set(),folders=new Set(),start=performance.now();
    while(queue.length){if(result.pages>=10000)throw Error('Page limit');const task=queue.shift(),key=JSON.stringify(task);if(seen.has(key))throw Error('Repeated cursor');seen.add(key);
      const u=new URL(a.url);u.searchParams.set('include_files',folderOnly?'false':'true');if(task.parent)u.searchParams.set('parent_directory_id',task.parent);if(task.cursor)u.searchParams.set('cursor',task.cursor);
      const b=page(await request(u.href));result.pages++;add(result,b.items,folderOnly);
      for(const f of b.items)if(typeof f.id==='string'&&f.id.startsWith('libdir_')&&f.trashed_at==null&&!folders.has(f.id)){folders.add(f.id);queue.push({parent:f.id,cursor:null});}
      if(b.cursor)queue.unshift({parent:task.parent,cursor:b.cursor});
    }
    result.ms=Math.round(performance.now()-start);return result;
  }
  async function flat(limit){
    const result=empty(),seen=new Set(),start=performance.now();let cursor=null;
    do{if(result.pages>=10000)throw Error('Page limit');const b=page(await request('/backend-api/files/library',{limit,cursor,include_hidden_files:true,trashed_only:false}));result.pages++;add(result,b.items);cursor=b.cursor;if(cursor&&seen.has(cursor))throw Error('Repeated cursor');seen.add(cursor);}while(cursor);
    result.ms=Math.round(performance.now()-start);return result;
  }
  function compare(left,right){const fields={},missing=[...left.keys()].filter(id=>!right.has(id)).length,extra=[...right.keys()].filter(id=>!left.has(id)).length;let changed=0;
    for(const [id,l] of left){const r=right.get(id);if(!r)continue;let differs=false;for(const k of Object.keys(l))if(l[k]!==r[k]){fields[k]=(fields[k]||0)+1;differs=true;}if(differs)changed++;}
    return {missing,extra,changed,fields};
  }
  const summary=r=>({files:r.files.size,folders:r.folders.size,pages:r.pages,duplicates:r.duplicates,unexpected_files:r.unexpected_files,ms:r.ms});
  const baseline=await nodes(false),modern=await flat(50),folders=await nodes(true);
  const probe100=await request('/backend-api/files/library',{limit:100,cursor:null,include_hidden_files:true,trashed_only:false});
  const repeated=await flat(50);
  return {baseline:summary(baseline),post50:summary(modern),folder_only:summary(folders),post50_repeat:summary(repeated),file_comparison:compare(baseline.files,modern.files),folder_comparison:compare(baseline.folders,folders.folders),repeat_comparison:compare(modern.files,repeated.files),post100:{status:probe100.status,items:Array.isArray(probe100.value?.items)?probe100.value.items.length:null},combined_candidate_ms:modern.ms+folders.ms};
})()
