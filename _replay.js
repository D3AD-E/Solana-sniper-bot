const https=require('https'); const zlib=require('zlib');
// grab a prefix of one hourly archive and decode as much of the zstd stream as it yields
function getRange(url, bytes){return new Promise((res,rej)=>{
  https.get(url,{headers:{Range:`bytes=0-${bytes}`}},r=>{
    if(r.statusCode>=400)return rej(new Error('HTTP '+r.statusCode));
    const c=[]; r.on('data',d=>c.push(d)); r.on('end',()=>res(Buffer.concat(c)));
  }).on('error',rej);})}
(async()=>{
  const url=process.argv[2];
  const buf=await getRange(url, 4*1024*1024);
  console.log('downloaded', (buf.length/1024/1024).toFixed(1),'MB of the archive');
  const stream=zlib.createZstdDecompress();
  let out=Buffer.alloc(0), lines=0;
  stream.on('data',d=>{out=Buffer.concat([out,d])});
  stream.on('error',()=>{});           // truncated stream is expected
  await new Promise(r=>{stream.on('end',r); stream.on('error',r); stream.end(buf);});
  const text=out.toString('utf8');
  const rows=text.split('\n').filter(Boolean);
  console.log('decoded', (out.length/1024/1024).toFixed(1),'MB ->', rows.length,'events\n');
  const kinds={};
  let sample=null, sampleTrade=null;
  for(const r of rows){
    let e; try{e=JSON.parse(r)}catch{continue}
    lines++;
    const k=e.type||e.event||e.kind||Object.keys(e)[0];
    kinds[k]=(kinds[k]||0)+1;
    if(!sample)sample=e;
    if(!sampleTrade && /trade|buy|swap/i.test(JSON.stringify(k)))sampleTrade=e;
  }
  console.log('event types:',JSON.stringify(kinds,null,0).slice(0,600));
  console.log('\nfirst event:\n',JSON.stringify(sample,null,1).slice(0,900));
  if(sampleTrade&&sampleTrade!==sample)console.log('\ntrade-ish event:\n',JSON.stringify(sampleTrade,null,1).slice(0,900));
})().catch(e=>console.error('ERR',e.message));
