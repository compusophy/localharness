#!/usr/bin/env node
// Generate web/feedback-resolutions.json — the apex-hosted feed the browser
// reads on mount to bell-notify a submitter that their on-chain feedback was
// resolved. Joins the RESOLVED index file (docs/feedback-resolved-mainnet.txt:
// index -> fixed-in version) with the on-chain FeedbackFacet (index -> sender +
// text, for the address match + a recognizable preview). MAINNET only. Read-only
// on-chain; best-effort (a network failure keeps the prior file). Node 18+.

import { readFileSync, writeFileSync, existsSync } from 'node:fs';

// ---- keccak-256 (copied from check-feedback.mjs, self-verified) ------------
const MASK = (1n << 64n) - 1n;
const RC = [0x0000000000000001n,0x0000000000008082n,0x800000000000808an,0x8000000080008000n,0x000000000000808bn,0x0000000080000001n,0x8000000080008081n,0x8000000000008009n,0x000000000000008an,0x0000000000000088n,0x0000000080008009n,0x000000008000000an,0x000000008000808bn,0x800000000000008bn,0x8000000000008089n,0x8000000000008003n,0x8000000000008002n,0x8000000000000080n,0x000000000000800an,0x800000008000000an,0x8000000080008081n,0x8000000000008080n,0x0000000080000001n,0x8000000080008008n];
const ROT=[[0,36,3,41,18],[1,44,10,45,2],[62,6,43,15,61],[28,55,25,21,56],[27,20,39,8,14]];
const rol=(x,n)=>((x<<BigInt(n))|(x>>BigInt(64-n)))&MASK;
function kf(A){for(let r=0;r<24;r++){const C=new Array(5);for(let x=0;x<5;x++)C[x]=A[x][0]^A[x][1]^A[x][2]^A[x][3]^A[x][4];const D=new Array(5);for(let x=0;x<5;x++)D[x]=C[(x+4)%5]^rol(C[(x+1)%5],1);for(let x=0;x<5;x++)for(let y=0;y<5;y++)A[x][y]^=D[x];const B=[[],[],[],[],[]];for(let x=0;x<5;x++)for(let y=0;y<5;y++)B[y][(2*x+3*y)%5]=rol(A[x][y],ROT[x][y]);for(let x=0;x<5;x++)for(let y=0;y<5;y++)A[x][y]=B[x][y]^(~B[(x+1)%5][y]&B[(x+2)%5][y]&MASK);A[0][0]^=RC[r];}}
function kc(b){const rate=136;const A=Array.from({length:5},()=>Array.from({length:5},()=>0n));const p=new Uint8Array(Math.ceil((b.length+1)/rate)*rate);p.set(b);p[b.length]^=0x01;p[p.length-1]^=0x80;for(let o=0;o<p.length;o+=rate){for(let i=0;i<rate/8;i++){let l=0n;for(let j=0;j<8;j++)l|=BigInt(p[o+i*8+j])<<BigInt(8*j);A[i%5][Math.floor(i/5)]^=l;}kf(A);}const out=new Uint8Array(32);for(let i=0;i<4;i++){let l=A[i%5][Math.floor(i/5)];for(let j=0;j<8;j++)out[i*8+j]=Number((l>>BigInt(8*j))&0xffn);}return out;}
const enc=s=>new TextEncoder().encode(s),hx=u=>[...u].map(b=>b.toString(16).padStart(2,'0')).join(''),sel=s=>hx(kc(enc(s)).slice(0,4));
if(hx(kc(enc('')))!=='c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470'){console.error('keccak self-test failed');process.exit(0);} // best-effort: never fail the build

const RPC='https://rpc.tempo.xyz', DIAMOND='0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77';
const OUT=new URL('../web/feedback-resolutions.json', import.meta.url);
const RESOLVED=new URL('../docs/feedback-resolved-mainnet.txt', import.meta.url);
const word=n=>BigInt(n).toString(16).padStart(64,'0');
const readWord=(h,off)=>BigInt('0x'+h.slice(off*2,off*2+64));

async function ethCall(data){
  const res=await fetch(RPC,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:1,method:'eth_call',params:[{to:DIAMOND,data:'0x'+data},'latest']})});
  const j=await res.json();
  if(j.error)throw new Error(JSON.stringify(j.error));
  return j.result.replace(/^0x/,'');
}
// decode feedbackRange -> [{index, sender, text}]
function decodeRange(h, start){
  const offS=Number(readWord(h,0)),offT=Number(readWord(h,32)),offX=Number(readWord(h,64));
  const nS=Number(readWord(h,offS)),senders=[];
  for(let k=0;k<nS;k++)senders.push('0x'+h.slice((offS+32+k*32)*2+24,(offS+32+k*32)*2+64));
  const nX=Number(readWord(h,offX)),base=offX+32,texts=[];
  for(let k=0;k<nX;k++){const elemOff=base+Number(readWord(h,base+k*32));const strLen=Number(readWord(h,elemOff));const bytes=h.slice((elemOff+32)*2,(elemOff+32)*2+strLen*2);texts.push(Buffer.from(bytes,'hex').toString('utf8'));}
  return senders.map((s,k)=>({index:start+k,sender:s.toLowerCase(),text:texts[k]}));
}

// resolved file: "index  fixed-in  note" (skip blank / # lines) -> index -> version
function loadResolved(){
  const map=new Map();
  if(!existsSync(RESOLVED))return map;
  for(const line of readFileSync(RESOLVED,'utf8').split('\n')){
    const t=line.trim();
    if(!t||t.startsWith('#'))continue;
    const parts=t.split(/\s+/);
    if(!/^\d+$/.test(parts[0]))continue;
    const idx=Number(parts[0]);
    const fixedIn=parts[1]||'';
    const version=/^\d+\.\d+\.\d+$/.test(fixedIn)?`v${fixedIn}`:''; // semver only; commits/(web) -> generic
    map.set(idx,version);
  }
  return map;
}
const preview=s=>(s||'').replace(/\s+/g,' ').replace(/^(feedback:?\s*)/i,'').trim().slice(0,70);

async function main(){
  const resolved=loadResolved();
  if(resolved.size===0){console.error('gen-feedback-resolutions: no resolved indices; writing []');writeFileSync(OUT,'[]\n');return;}
  let count;
  try{count=Number(BigInt('0x'+await ethCall(sel('feedbackCount()'))));}
  catch(e){console.error('gen-feedback-resolutions: feedbackCount failed ('+e.message+'); keeping existing file');return;}
  const all=[];const PAGE=40;
  for(let start=0;start<count;start+=PAGE){
    const data=sel('feedbackRange(uint256,uint256)')+word(start)+word(Math.min(PAGE,count-start));
    all.push(...decodeRange(await ethCall(data),start));
  }
  const out=[];
  for(const e of all){
    if(!resolved.has(e.index))continue;
    out.push({index:e.index, sender:e.sender, version:resolved.get(e.index), preview:preview(e.text)});
  }
  writeFileSync(OUT, JSON.stringify(out)+'\n');
  console.error(`gen-feedback-resolutions: wrote ${out.length} resolved entries for ${count} on-chain feedback items`);
}
main().catch(e=>{console.error('gen-feedback-resolutions: '+e.message+' (non-fatal)');});
