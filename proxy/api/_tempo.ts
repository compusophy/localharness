// Tempo 0x76 transaction — the fee-payer half, server-side.
//
// The rate-capped sponsor RELAY (`api/sponsor.ts`, design/cli-mainnet-relay.md
// §2.2) signs the fee_payer authorization of a user's Tempo 0x76 tx so the
// published CLI ships NO money-moving key. The relay needs exactly ONE thing
// from the wire format: the fee_payer SIGNING HASH
//
//   keccak256(0x78 || rlp([
//     chain_id, mpfpg, mfpg, gas_limit, calls, access_list,
//     nonce_key, nonce, valid_before, valid_after,
//     fee_token, sender_address, aa_authorization_list
//   ]))
//
// This is the EXACT TS mirror of `src/tempo_tx.rs::fee_payer_hash` (which was
// live-proven against Tempo Moderato and is pinned by GOLDEN_FEE_PAYER_HASH).
// `test/tempo-feepayer.mjs` re-derives that golden hash from this module — if
// the Rust wire format moves, that parity test fails here too.
//
// The relay deliberately does NOT serialize the full signed tx: it returns the
// raw 65-byte fee_payer signature and the CLI plugs it into the tested Rust
// `serialize_signed` (which does the rlp([v,r,s]) minimal-int wrapping). So this
// module owns only: RLP encode → fee_payer hash → sign.

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';

// --- RLP -------------------------------------------------------------------
// Minimal RLP encoder (byte strings + lists), matching `src/wallet.rs`'s
// rlp_bytes / rlp_list / rlp_uint. Integers are MINIMAL big-endian (leading
// zeros stripped; zero → the empty string 0x80).

function rlpBytes(b: Uint8Array): Uint8Array {
  if (b.length === 1 && b[0] < 0x80) return b; // single low byte is its own RLP
  if (b.length <= 55) return concat(Uint8Array.of(0x80 + b.length), b);
  const len = lenBytes(b.length);
  return concat(concat(Uint8Array.of(0xb7 + len.length), len), b);
}

function rlpList(items: Uint8Array[]): Uint8Array {
  const body = items.reduce((acc, it) => concat(acc, it), new Uint8Array(0));
  if (body.length <= 55) return concat(Uint8Array.of(0xc0 + body.length), body);
  const len = lenBytes(body.length);
  return concat(concat(Uint8Array.of(0xf7 + len.length), len), body);
}

/** RLP of an unsigned integer as a minimal big-endian byte string. */
function rlpUint(n: bigint): Uint8Array {
  return rlpBytes(uintToMinimalBytes(n));
}

/** Minimal big-endian bytes for a non-negative bigint; empty for 0. */
function uintToMinimalBytes(n: bigint): Uint8Array {
  if (n < 0n) throw new Error('rlp uint must be non-negative');
  if (n === 0n) return new Uint8Array(0);
  let hex = n.toString(16);
  if (hex.length % 2) hex = '0' + hex;
  return hexToBytes(hex);
}

/** Big-endian length prefix for RLP long-form headers (no leading zeros). */
function lenBytes(n: number): Uint8Array {
  return uintToMinimalBytes(BigInt(n));
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

// --- intent ----------------------------------------------------------------

/** One inner call in a Tempo tx's `calls[]`. */
export interface TempoCallIntent {
  to: Uint8Array; // 20 bytes
  value: bigint;
  input: Uint8Array; // calldata
}

/** The sender-authorized intent fields the fee_payer commits to. */
export interface TempoIntent {
  chainId: bigint;
  maxPriorityFeePerGas: bigint;
  maxFeePerGas: bigint;
  gasLimit: bigint;
  calls: TempoCallIntent[];
  nonceKey: bigint;
  nonce: bigint;
  validBefore: bigint | null;
  validAfter: bigint | null;
  feeToken: Uint8Array; // 20 bytes (real token — sponsored fee_token)
  /** CONTRACT CREATION: each call's `to` is RLP-encoded EMPTY (0x80, the EVM
   * null-target convention) so `input` runs as init-code. Mirrors
   * `TempoTx.create` / `rlp_create_call` (pinned by GOLDEN_CREATE_SENDER_HASH);
   * optional — absent/false = plain calls (every pre-existing intent). */
  create?: boolean;
}

/** The 10-item prefix shared by sender hash, fee_payer hash, and the tx body. */
function commonRlpItems(tx: TempoIntent): Uint8Array[] {
  const callItems = tx.calls.map((c) =>
    rlpList([
      rlpBytes(tx.create ? new Uint8Array(0) : c.to),
      rlpUint(c.value),
      rlpBytes(c.input),
    ]),
  );
  const optInt = (v: bigint | null) =>
    v === null ? rlpBytes(new Uint8Array(0)) : rlpUint(v);
  return [
    rlpUint(tx.chainId),
    rlpUint(tx.maxPriorityFeePerGas),
    rlpUint(tx.maxFeePerGas),
    rlpUint(tx.gasLimit),
    rlpList(callItems),
    rlpList([]), // access_list — empty for our usage (0xc0)
    rlpUint(tx.nonceKey),
    rlpUint(tx.nonce),
    optInt(tx.validBefore),
    optInt(tx.validAfter),
  ];
}

/**
 * The fee_payer signing hash: `keccak256(0x78 || rlp([common.., fee_token,
 * sender_address, aa_authorization_list]))`. `senderAddress` is 20 bytes — the
 * address that signed the SENDER half (the caller's identity). aa_authorization
 * list is the empty list (0xc0); key_authorization is OMITTED when None (we
 * never set it). Exact mirror of `TempoTx::fee_payer_hash`.
 */
export function feePayerHash(tx: TempoIntent, senderAddress: Uint8Array): Uint8Array {
  if (senderAddress.length !== 20) throw new Error('sender address must be 20 bytes');
  if (tx.feeToken.length !== 20) throw new Error('fee_token must be 20 bytes');
  const items = commonRlpItems(tx);
  items.push(rlpBytes(tx.feeToken));
  items.push(rlpBytes(senderAddress));
  items.push(rlpList([])); // aa_authorization_list (empty)
  const body = rlpList(items);
  const payload = concat(Uint8Array.of(0x78), body);
  return keccak_256(payload);
}

/**
 * The SENDER signing hash for a SPONSORED tx: `keccak256(0x76 || rlp([common..,
 * 0x80 (fee_token empty), 0x00 (fee_payer-sig placeholder), aa_authorization_
 * list]))`. The relay uses this to VERIFY the caller's `senderSignature`
 * recovers `senderAddress` — i.e. the caller really authorized this exact intent
 * (no blind fee-payer signing). Mirror of the sponsored branch of
 * `TempoTx::sender_hash`.
 */
export function sponsoredSenderHash(tx: TempoIntent): Uint8Array {
  const items = commonRlpItems(tx);
  items.push(rlpBytes(new Uint8Array(0))); // fee_token → 0x80 (empty)
  items.push(Uint8Array.of(0x00)); // fee_payer_sig slot → literal 0x00 placeholder
  items.push(rlpList([])); // aa_authorization_list (empty)
  const body = rlpList(items);
  const payload = concat(Uint8Array.of(0x76), body);
  return keccak_256(payload);
}

// --- signing & recovery ----------------------------------------------------

/**
 * Sign a 32-byte digest with `privKeyHex`, returning the 65-byte `r||s||v`
 * signature with v ∈ {27,28} — the convention `src/wallet.rs::sign_hash`
 * produces and `serialize_signed` consumes. noble emits low-s by default, which
 * the chain requires.
 */
export function signHash65(digest: Uint8Array, privKeyHex: string): Uint8Array {
  const sig = secp256k1.sign(digest, hexToBytes(stripHex(privKeyHex)));
  const out = new Uint8Array(65);
  out.set(hexToBytes(sig.r.toString(16).padStart(64, '0')), 0);
  out.set(hexToBytes(sig.s.toString(16).padStart(64, '0')), 32);
  out[64] = 27 + sig.recovery;
  return out;
}

// secp256k1n / 2 — EIP-2 low-s bound (matches X402Facet.HALF_N + _authcore.ts +
// the Rust recover_address gate). Reject the malleable high-s twin (audit I3).
const SECP256K1_HALF_N =
  0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0n;

/** Lowercase 0x address from a 65-byte (r||s||v) sig over `digest`. */
export function recoverAddressFromDigest(sig65: Uint8Array, digest: Uint8Array): string {
  if (sig65.length !== 65) throw new Error('signature must be 65 bytes');
  if (BigInt('0x' + bytesToHex(sig65.slice(32, 64))) > SECP256K1_HALF_N) {
    throw new Error('signature has high-s (EIP-2 malleable) — not accepted');
  }
  let v = sig65[64];
  if (v >= 27) v -= 27;
  const signature = secp256k1.Signature.fromCompact(
    bytesToHex(sig65.slice(0, 64)),
  ).addRecoveryBit(v);
  const point = signature.recoverPublicKey(digest);
  return '0x' + bytesToHex(keccak_256(point.toRawBytes(false).slice(1)).slice(12));
}

/** Lowercase 0x address derived from a private key. */
export function addressFromPrivKey(privKeyHex: string): string {
  const pub = secp256k1.getPublicKey(hexToBytes(stripHex(privKeyHex)), false); // 65 bytes, 0x04 prefix
  return '0x' + bytesToHex(keccak_256(pub.slice(1)).slice(12));
}

export function stripHex(h: string): string {
  return h.startsWith('0x') ? h.slice(2) : h;
}

export { bytesToHex, hexToBytes };
