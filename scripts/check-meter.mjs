#!/usr/bin/env node
// Diagnostic: owner + creditOf (meter) for one or more agent names on MAINNET,
// queried the SAME way the proxy gate does. Read-only.
// Usage: node scripts/check-meter.mjs frank krafto claude
const MASK=(1n<<64n)-1n;
const RC=[0x0000000000000001n,0x0000000000008082n,0x800000000000808an,0x8000000080008000n,0x000000000000808bn,0x0000000080000001n,0x8000000080008081n,0x8000000000008009n,0x000000000000008an,0x0000000000000088n,0x0000000080008009n,0x000000008000000an,0x000000008000808bn,0x800000000000008bn,0x8000000000008089n,0x8000000000008003n,0x8000000000008002n,0x8000000000000080n,0x000000000000800an,0x800000008000000an,0x8000000080008081n,0x8000000000008080n,0x0000000080000001n,0x8000000080008008n];
const ROT=[[0,36,3,41,18],[1,44,10,45,2],[62,6,43,15,61],[28,55,25,21,56],[27,20,39,8,14]];
const rol=(x,n)=>((x<<BigInt(n))|(x>>BigInt(64-n)))&MASK;
function kf(A){for(let r=0;r<24;r++){const C=new Array(5);for(let x=0;x<5;x++)C[x]=A[x][0]^A[x][1]^A[x][2]^A[x][3]^A[x][4];const D=new Array(5);for(let x=0;x<5;x++)D[x]=C[(x+4)%5]^rol(C[(x+1)%5],1);for(let x=0;x<5;x++)for(let y=0;y<5;y++)A[x][y]^=D[x];const B=[[],[],[],[],[]];for(let x=0;x<5;x++)for(let y=0;y<5;y++)B[y][(2*x+3*y)%5]=rol(A[x][y],ROT[x][y]);for(let x=0;x<5;x++)for(let y=0;y<5;y++)A[x][y]=B[x][y]^(~B[(x+1)%5][y]&B[(x+2)%5][y]&MASK);A[0][0]^=RC[r];}}
function kc(b){const rate=136;const A=Array.from({length:5},()=>Array.from({length:5},()=>0n));const p=new Uint8Array(Math.ceil((b.length+1)/rate)*rate);p.set(b);p[b.length]^=0x01;p[p.length-1]^=0x80;for(let o=0;o<p.length;o+=rate){for(let i=0;i<rate/8;i++){let l=0n;for(let j=0;j<8;j++)l|=BigInt(p[o+i*8+j])<<BigInt(8*j);A[i%5][Math.floor(i/5)]^=l;}kf(A);}const out=new Uint8Array(32);for(let i=0;i<4;i++){let l=A[i%5][Math.floor(i/5)];for(let j=0;j<8;j++)out[i*8+j]=Number((l>>BigInt(8*j))&0xffn);}return out;}
const enc=s=>new TextEncoder().encode(s),hx=u=>[...u].map(b=>b.toString(16).padStart(2,'0')).join(''),sel=s=>hx(kc(enc(s)).slice(0,4));
const RPC='https://rpc.tempo.xyz',D='0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77',LH='0x7ba3c9a39596e438b05c56dfc779700b58aea814';
async function ec(to,data){const t0=Date.now();const r=await fetch(RPC,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:1,method:'eth_call',params:[{to,data:'0x'+data},'latest']})});const j=await r.json();const ms=Date.now()-t0;if(j.error)throw new Error(JSON.stringify(j.error));return {hex:j.result.replace(/^0x/,''),ms};}
const aw=a=>a.replace(/^0x/,'').toLowerCase().padStart(64,'0'),w=n=>BigInt(n).toString(16).padStart(64,'0');
const fmt=h=>{const v=BigInt('0x'+(h||'0'));return `${v/(10n**18n)}.${(v%(10n**18n)).toString().padStart(18,'0').slice(0,2)} $LH`;};
const names=process.argv.slice(2);if(!names.length)names.push('frank','krafto');
for(const name of names){
  const nb=enc(name);
  const od=sel('ownerOfName(string)')+w(32)+w(nb.length)+hx(nb).padEnd(64,'0');
  try{
    const {hex:or}=await ec(D,od);const owner='0x'+or.slice(24,64);
    const {hex:cr,ms}=await ec(D,sel('creditOf(address)')+aw(owner));
    const {hex:wr}=await ec(LH,sel('balanceOf(address)')+aw(owner));
    console.log(`${name.padEnd(10)} owner=${owner}  METER(creditOf)=${fmt(cr)}  WALLET=${fmt(wr)}  [creditOf read ${ms}ms]`);
  }catch(e){console.log(`${name.padEnd(10)} ERROR ${e.message}`);}
}
