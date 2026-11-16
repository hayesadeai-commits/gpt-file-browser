/**
 * Compare a reviewed snapshot (f) with current remote metadata (n).
 * Returns the first changed field name, or null if the available checks pass.
 * This file is a function expression, not a standalone module: changes.rs embeds
 * it into its browser script, and the Node tests execute this exact expression.
 * Do not add side effects or weaken ownership checks to accommodate missing dates.
 */
(f,n)=>{
  if(n.id!==f.id)return 'identity';
  if(n.access_kind!=='owned')return 'ownership';
  if((n.name??n.file_name)!==f.name)return 'name';
  if(n.file_size_bytes!==f.bytes)return 'size';
  if((n.parent_directory_id??n.directory_id??'')!==f.parent)return 'folder';
  // A changed content ID is a new file version even when name and size are unchanged.
  if(f.file_id&&n.file_id&&n.file_id!==f.file_id)return 'file version';
  // Older Library nodes omit updated_at even when the listing has it.
  // Missing evidence is not evidence of a modification.
  if(f.modified&&n.updated_at){
    // Accept equivalent timestamp encodings; Date.parse compares at millisecond precision.
    // If parsing is unavailable, require the original string values to match.
    const before=Date.parse(f.modified),now=Date.parse(n.updated_at);
    if(Number.isFinite(before)&&Number.isFinite(now)?before!==now:f.modified!==n.updated_at)return 'modified time';
  }
  return null;
}
