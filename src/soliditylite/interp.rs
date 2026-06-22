//! SolidityLite EVM interpreter — a MINIMAL, dependency-free EVM-subset executor
//! used as a DIFF-HARNESS for the [`super::codegen`] emitter (issue #37).
//!
//! This is NOT a general-purpose EVM (no `revm` — the user rejected the heavy dep;
//! we build our OWN minimal interpreter, the same "our own language subset"
//! philosophy as [`super::asm`]/[`super::codegen`]). It executes EXACTLY the opcode
//! set SolidityLite emits (a stack machine + memory + storage), so a compiled facet
//! can be deployed-and-called entirely in-process and its results asserted against
//! the known-good behavior of the shipped features. If the interpreter cannot
//! reproduce a known-good case, the interpreter is wrong — that bootstrap is what
//! makes the harness trustworthy (see the `tests` module).
//!
//! ## Supported opcodes
//!
//! The full set [`super::codegen`] + [`super::asm::init_wrapper`] emit: `PUSH1..32`,
//! `POP`, `DUP1`, `MSTORE`/`MLOAD`, `SLOAD`/`SSTORE`, `CALLDATASIZE`/`CALLDATALOAD`,
//! `LT`/`GT`/`EQ`/`ISZERO`/`SHR`, `ADD`/`SUB`/`MUL`/`DIV`/`MOD`, `KECCAK256`,
//! `CALLER`/`TIMESTAMP`/`NUMBER`, `JUMP`/`JUMPI`/`JUMPDEST`, `LOG0..LOG4`,
//! `CODECOPY`, `RETURN`/`REVERT`. Any opcode OUTSIDE this set is an
//! [`ExecError::UnknownOpcode`] — the harness never silently no-ops an unhandled
//! instruction (which would mask a codegen bug).
//!
//! `KECCAK256`, storage-slot keccak derivation, the `CALLDATALOAD` word read, and
//! the `RETURN(off,len)` ABI word are the tricky parts the codegen relies on; they
//! are implemented to match the real EVM exactly (gated on `wallet` for `sha3`).

#![cfg(feature = "wallet")]

use crate::soliditylite::asm::op;
use std::collections::HashMap;
use sha3::{Digest, Keccak256};

/// A 256-bit EVM word, big-endian (the same `[u8; 32]` shape codegen uses for
/// slots/values).
pub type Word = [u8; 32];

/// Why execution halted abnormally (anything other than a clean `RETURN`/`STOP`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// The contract executed `REVERT(off, len)` — the returned data (often empty).
    Revert(Vec<u8>),
    /// An opcode outside the SolidityLite subset was hit (a codegen bug or an
    /// unsupported feature — never silently ignored).
    UnknownOpcode(u8),
    /// The stack underflowed (popped more than was pushed) — a codegen bug.
    StackUnderflow,
    /// A `JUMP`/`JUMPI` targeted an offset that is not a `JUMPDEST` — a codegen bug.
    BadJumpDest(usize),
    /// The execution budget (a loop/runaway guard) was exhausted.
    OutOfGas,
}

/// The result of a successful call: the `RETURN`ed data (possibly empty).
pub type ExecResult = Result<Vec<u8>, ExecError>;

/// A persistent contract account: its deployed runtime bytecode + word→word
/// storage. Construct via [`Contract::deploy`] (runs INIT code, EVM-style) so the
/// harness exercises the SAME `CODECOPY`/`RETURN` constructor the chain runs.
#[derive(Debug, Clone, Default)]
pub struct Contract {
    /// The deployed runtime bytecode (what the chain stores + runs on each call).
    pub code: Vec<u8>,
    /// Persistent storage: slot → 32-byte word. Missing slots read as zero.
    pub storage: HashMap<Word, Word>,
}

/// The transaction-like context for one call: who is calling and the block env.
/// Mirrors the only environment reads codegen emits (`CALLER`/`TIMESTAMP`/`NUMBER`).
#[derive(Debug, Clone, Default)]
pub struct CallEnv {
    /// `msg.sender` (`CALLER`) — a 20-byte address, left-padded to a word on read.
    pub caller: [u8; 20],
    /// `block.timestamp` (`TIMESTAMP`).
    pub timestamp: u64,
    /// `block.number` (`NUMBER`).
    pub number: u64,
}

/// An emitted log (`LOGn`): the topics (`topic0..`) and the data region bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The log topics, `topic0` first (1..=4 for the events SolidityLite emits).
    pub topics: Vec<Word>,
    /// The log data region (`mem[offset..offset+len]`).
    pub data: Vec<u8>,
}

/// Hard cap on executed instructions — a runaway/loop guard (SolidityLite bodies
/// are straight-line + bounded branches, so any real program finishes far under
/// this; an infinite loop would mean a codegen bug, caught as [`ExecError::OutOfGas`]).
const STEP_BUDGET: usize = 1_000_000;

impl Contract {
    /// "Deploy" INIT code EVM-style: run it, and the bytes it `RETURN`s become the
    /// contract's deployed `code`. This mirrors a CREATE tx, so the harness covers
    /// [`super::asm::init_wrapper`]'s `CODECOPY`/`RETURN` constructor too. Storage
    /// starts empty.
    pub fn deploy(init_code: &[u8], env: &CallEnv) -> Result<Contract, ExecError> {
        let mut c = Contract { code: Vec::new(), storage: HashMap::new() };
        // The constructor runs with the INIT code AS its code and empty calldata.
        let runtime = run(init_code, &[], env, &mut c.storage, &mut Vec::new())?;
        c.code = runtime;
        Ok(c)
    }

    /// Execute a call against this contract's deployed `code` with the given
    /// `calldata` (selector ++ ABI args), mutating storage. Returns the `RETURN`ed
    /// bytes or an [`ExecError`]. Emitted logs are discarded (use [`Contract::call_logs`]).
    pub fn call(&mut self, calldata: &[u8], env: &CallEnv) -> ExecResult {
        let code = self.code.clone();
        run(&code, calldata, env, &mut self.storage, &mut Vec::new())
    }

    /// Like [`Contract::call`] but also returns the logs emitted during the call.
    pub fn call_logs(&mut self, calldata: &[u8], env: &CallEnv) -> Result<(Vec<u8>, Vec<LogEntry>), ExecError> {
        let code = self.code.clone();
        let mut logs = Vec::new();
        let ret = run(&code, calldata, env, &mut self.storage, &mut logs)?;
        Ok((ret, logs))
    }

    /// Read storage slot `slot` (zero if never written) — for asserting post-state
    /// in the diff-harness without going through a getter.
    pub fn sload(&self, slot: &Word) -> Word {
        self.storage.get(slot).copied().unwrap_or([0u8; 32])
    }
}

/// Build a `selector ++ args` calldata blob (each arg a 32-byte word).
pub fn calldata(selector: [u8; 4], args: &[Word]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 * args.len());
    out.extend_from_slice(&selector);
    for a in args {
        out.extend_from_slice(a);
    }
    out
}

/// A `u64` as a big-endian 32-byte word (an ABI `uint256` argument / return value).
pub fn word(v: u64) -> Word {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// A 20-byte address as a left-padded 32-byte word (an ABI `address` argument).
pub fn addr_word(a: &[u8; 20]) -> Word {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

/// Decode a 32-byte BE word as a `u64` (its low 8 bytes) — for reading a `uint256`
/// return value small enough to fit (lengths, counts, the harness's test values).
pub fn word_to_u64(w: &Word) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&w[24..]);
    u64::from_be_bytes(b)
}

/// The big-endian `u256` add of two words (wrapping mod 2^256), used by the
/// interpreter for `ADD` and slot+index derivation (matches the EVM).
fn add256(a: &Word, b: &Word) -> Word {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in (0..32).rev() {
        let v = a[i] as u16 + b[i] as u16 + carry;
        out[i] = (v & 0xFF) as u8;
        carry = v >> 8;
    }
    out
}

/// The big-endian `u256` subtract `a - b` (wrapping mod 2^256), used for `SUB`.
fn sub256(a: &Word, b: &Word) -> Word {
    let mut out = [0u8; 32];
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let v = a[i] as i16 - b[i] as i16 - borrow;
        if v < 0 {
            out[i] = (v + 256) as u8;
            borrow = 1;
        } else {
            out[i] = v as u8;
            borrow = 0;
        }
    }
    out
}

/// The execution stack/memory + a program counter, run to completion.
struct Vm<'a> {
    code: &'a [u8],
    calldata: &'a [u8],
    env: &'a CallEnv,
    storage: &'a mut HashMap<Word, Word>,
    logs: &'a mut Vec<LogEntry>,
    stack: Vec<Word>,
    /// Byte-addressed memory, grown on demand (zero-filled).
    memory: Vec<u8>,
    pc: usize,
}

/// Execute `code` with `calldata`, returning the `RETURN`ed bytes (or an error).
/// `storage` is read+written in place; `logs` accumulates any `LOGn`.
fn run(
    code: &[u8],
    calldata: &[u8],
    env: &CallEnv,
    storage: &mut HashMap<Word, Word>,
    logs: &mut Vec<LogEntry>,
) -> ExecResult {
    let mut vm = Vm {
        code,
        calldata,
        env,
        storage,
        logs,
        stack: Vec::new(),
        memory: Vec::new(),
        pc: 0,
    };
    vm.exec()
}

impl Vm<'_> {
    fn pop(&mut self) -> Result<Word, ExecError> {
        self.stack.pop().ok_or(ExecError::StackUnderflow)
    }

    /// Ensure `memory` covers `[off, off+len)`, zero-extending as needed.
    fn ensure_mem(&mut self, off: usize, len: usize) {
        let end = off.saturating_add(len);
        if end > self.memory.len() {
            self.memory.resize(end, 0);
        }
    }

    /// Store a 32-byte word at memory offset `off`.
    fn mstore(&mut self, off: usize, w: &Word) {
        self.ensure_mem(off, 32);
        self.memory[off..off + 32].copy_from_slice(w);
    }

    /// Load a 32-byte word from memory offset `off` (zero-extended past the end).
    fn mload(&mut self, off: usize) -> Word {
        self.ensure_mem(off, 32);
        let mut w = [0u8; 32];
        w.copy_from_slice(&self.memory[off..off + 32]);
        w
    }

    /// Read `len` calldata bytes starting at `off`, zero-extended past the end —
    /// the exact `CALLDATALOAD` word semantics (a read past calldata reads zeros).
    fn calldataword(&self, off: usize) -> Word {
        let mut w = [0u8; 32];
        for (i, byte) in w.iter_mut().enumerate() {
            let src = off.wrapping_add(i);
            if src < self.calldata.len() {
                *byte = self.calldata[src];
            }
        }
        w
    }

    fn exec(&mut self) -> ExecResult {
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > STEP_BUDGET {
                return Err(ExecError::OutOfGas);
            }
            if self.pc >= self.code.len() {
                // Running off the end is an implicit STOP (empty return).
                return Ok(Vec::new());
            }
            let opc = self.code[self.pc];
            match opc {
                // PUSH1..PUSH32: read the opcode's immediate operand onto the stack,
                // right-aligned into a 32-byte word.
                o if (op::PUSH1..=op::PUSH1 + 31).contains(&o) => {
                    let n = (o - op::PUSH1) as usize + 1;
                    let start = self.pc + 1;
                    let mut w = [0u8; 32];
                    for i in 0..n {
                        let b = self.code.get(start + i).copied().unwrap_or(0);
                        w[32 - n + i] = b;
                    }
                    self.stack.push(w);
                    self.pc += 1 + n;
                }
                op::POP => {
                    self.pop()?;
                    self.pc += 1;
                }
                op::DUP1 => {
                    let top = *self.stack.last().ok_or(ExecError::StackUnderflow)?;
                    self.stack.push(top);
                    self.pc += 1;
                }
                op::ADD => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.stack.push(add256(&a, &b));
                    self.pc += 1;
                }
                op::SUB => {
                    // SUB = μs[0] - μs[1] (top minus next).
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.stack.push(sub256(&a, &b));
                    self.pc += 1;
                }
                op::MUL => {
                    let a = word_to_u128(&self.pop()?);
                    let b = word_to_u128(&self.pop()?);
                    self.stack.push(u128_to_word(a.wrapping_mul(b)));
                    self.pc += 1;
                }
                op::DIV => {
                    let a = word_to_u128(&self.pop()?);
                    let b = word_to_u128(&self.pop()?);
                    self.stack.push(u128_to_word(if b == 0 { 0 } else { a / b }));
                    self.pc += 1;
                }
                op::MOD => {
                    let a = word_to_u128(&self.pop()?);
                    let b = word_to_u128(&self.pop()?);
                    self.stack.push(u128_to_word(if b == 0 { 0 } else { a % b }));
                    self.pc += 1;
                }
                op::LT => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.stack.push(bool_word(cmp256(&a, &b).is_lt()));
                    self.pc += 1;
                }
                op::GT => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.stack.push(bool_word(cmp256(&a, &b).is_gt()));
                    self.pc += 1;
                }
                op::EQ => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.stack.push(bool_word(a == b));
                    self.pc += 1;
                }
                op::ISZERO => {
                    let a = self.pop()?;
                    self.stack.push(bool_word(a == [0u8; 32]));
                    self.pc += 1;
                }
                op::SHR => {
                    // SHR(shift, value) = value >> shift (top = shift, next = value).
                    let shift = word_to_u128(&self.pop()?);
                    let value = self.pop()?;
                    self.stack.push(shr256(&value, shift));
                    self.pc += 1;
                }
                op::KECCAK256 => {
                    // KECCAK256(offset, len) over memory.
                    let off = word_to_usize(&self.pop()?);
                    let len = word_to_usize(&self.pop()?);
                    self.ensure_mem(off, len);
                    let digest = Keccak256::digest(&self.memory[off..off + len]);
                    let mut w = [0u8; 32];
                    w.copy_from_slice(&digest);
                    self.stack.push(w);
                    self.pc += 1;
                }
                op::MSTORE => {
                    let off = word_to_usize(&self.pop()?);
                    let val = self.pop()?;
                    self.mstore(off, &val);
                    self.pc += 1;
                }
                0x51 => {
                    // MLOAD(off) — not emitted by codegen but cheap + correct to support.
                    let off = word_to_usize(&self.pop()?);
                    let w = self.mload(off);
                    self.stack.push(w);
                    self.pc += 1;
                }
                op::SLOAD => {
                    let slot = self.pop()?;
                    let v = self.storage.get(&slot).copied().unwrap_or([0u8; 32]);
                    self.stack.push(v);
                    self.pc += 1;
                }
                op::SSTORE => {
                    let slot = self.pop()?;
                    let val = self.pop()?;
                    if val == [0u8; 32] {
                        self.storage.remove(&slot);
                    } else {
                        self.storage.insert(slot, val);
                    }
                    self.pc += 1;
                }
                op::CALLDATASIZE => {
                    self.stack.push(word(self.calldata.len() as u64));
                    self.pc += 1;
                }
                op::CALLDATALOAD => {
                    let off = word_to_usize(&self.pop()?);
                    let w = self.calldataword(off);
                    self.stack.push(w);
                    self.pc += 1;
                }
                op::CALLER => {
                    self.stack.push(addr_word(&self.env.caller));
                    self.pc += 1;
                }
                op::TIMESTAMP => {
                    self.stack.push(word(self.env.timestamp));
                    self.pc += 1;
                }
                op::NUMBER => {
                    self.stack.push(word(self.env.number));
                    self.pc += 1;
                }
                op::CODECOPY => {
                    // CODECOPY(destOff, codeOff, len): copy this code into memory.
                    let dest = word_to_usize(&self.pop()?);
                    let src = word_to_usize(&self.pop()?);
                    let len = word_to_usize(&self.pop()?);
                    self.ensure_mem(dest, len);
                    for i in 0..len {
                        let b = self.code.get(src + i).copied().unwrap_or(0);
                        self.memory[dest + i] = b;
                    }
                    self.pc += 1;
                }
                op::JUMP => {
                    let dest = word_to_usize(&self.pop()?);
                    self.jump(dest)?;
                }
                op::JUMPI => {
                    let dest = word_to_usize(&self.pop()?);
                    let cond = self.pop()?;
                    if cond != [0u8; 32] {
                        self.jump(dest)?;
                    } else {
                        self.pc += 1;
                    }
                }
                op::JUMPDEST => {
                    self.pc += 1;
                }
                op::RETURN => {
                    let off = word_to_usize(&self.pop()?);
                    let len = word_to_usize(&self.pop()?);
                    self.ensure_mem(off, len);
                    return Ok(self.memory[off..off + len].to_vec());
                }
                op::REVERT => {
                    let off = word_to_usize(&self.pop()?);
                    let len = word_to_usize(&self.pop()?);
                    self.ensure_mem(off, len);
                    return Err(ExecError::Revert(self.memory[off..off + len].to_vec()));
                }
                o if (op::LOG0..=op::LOG4).contains(&o) => {
                    let ntopics = (o - op::LOG0) as usize;
                    let off = word_to_usize(&self.pop()?);
                    let len = word_to_usize(&self.pop()?);
                    let mut topics = Vec::with_capacity(ntopics);
                    for _ in 0..ntopics {
                        topics.push(self.pop()?);
                    }
                    self.ensure_mem(off, len);
                    let data = self.memory[off..off + len].to_vec();
                    self.logs.push(LogEntry { topics, data });
                    self.pc += 1;
                }
                other => return Err(ExecError::UnknownOpcode(other)),
            }
        }
    }

    /// Jump to `dest`, requiring it to land on a `JUMPDEST` (real EVM rule). A jump
    /// into a `PUSH` immediate or off the end is a [`ExecError::BadJumpDest`].
    fn jump(&mut self, dest: usize) -> Result<(), ExecError> {
        if self.code.get(dest) != Some(&op::JUMPDEST) {
            return Err(ExecError::BadJumpDest(dest));
        }
        self.pc = dest;
        Ok(())
    }
}

/// `1`/`0` as a 32-byte word (the EVM boolean encoding).
fn bool_word(b: bool) -> Word {
    let mut w = [0u8; 32];
    if b {
        w[31] = 1;
    }
    w
}

/// Unsigned big-endian word comparison (the EVM `LT`/`GT` are unsigned).
fn cmp256(a: &Word, b: &Word) -> std::cmp::Ordering {
    a.iter().cmp(b.iter())
}

/// A logical right shift of a 256-bit word by `shift` bits (`SHR`).
fn shr256(value: &Word, shift: u128) -> Word {
    if shift >= 256 {
        return [0u8; 32];
    }
    let shift = shift as usize;
    let byte_shift = shift / 8;
    let bit_shift = shift % 8;
    let mut out = [0u8; 32];
    // Shift right by whole bytes first (toward higher indices = less significant).
    for (i, byte) in out.iter_mut().enumerate() {
        let src = i as isize - byte_shift as isize;
        if src >= 0 {
            *byte = value[src as usize];
        }
    }
    if bit_shift > 0 {
        let mut carry = 0u8;
        for byte in out.iter_mut() {
            let new_carry = *byte << (8 - bit_shift);
            *byte = (*byte >> bit_shift) | carry;
            carry = new_carry;
        }
    }
    out
}

/// The low 128 bits of a word as a `u128` (SolidityLite's test values + MUL/DIV/MOD
/// operands all fit; the high 128 bits are ignored — sufficient for the harness's
/// bounded arithmetic, NOT a full 256-bit multiply).
fn word_to_u128(w: &Word) -> u128 {
    let mut b = [0u8; 16];
    b.copy_from_slice(&w[16..]);
    u128::from_be_bytes(b)
}

/// A `u128` as the low 128 bits of a 32-byte word (high half zero).
fn u128_to_word(v: u128) -> Word {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// A word as a `usize` memory/calldata offset (its low bytes; offsets are tiny).
fn word_to_usize(w: &Word) -> usize {
    word_to_u64(w) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soliditylite::compile;

    /// Compile SolidityLite source and deploy it into a fresh interpreter Contract.
    fn deploy_src(src: &str) -> Contract {
        let art = compile(src).expect("source must compile");
        Contract::deploy(&art.init_code, &CallEnv::default()).expect("deploy must succeed")
    }

    /// Compute a selector the same way the compiler does (the ABI canonical sig).
    fn sel(sig: &str) -> [u8; 4] {
        crate::registry::selector(sig)
    }

    // ── arithmetic-primitive unit tests (the tricky word ops) ────────────────

    #[test]
    fn add_sub_wrap_mod_2_256() {
        let max = [0xffu8; 32];
        let one = word(1);
        assert_eq!(add256(&max, &one), [0u8; 32], "max + 1 wraps to 0");
        assert_eq!(sub256(&[0u8; 32], &one), max, "0 - 1 wraps to max");
    }

    #[test]
    fn shr_by_224_extracts_a_selector() {
        // A selector sits in the HIGH 4 bytes of the calldata's first word; SHR 224
        // brings it to the low 4 bytes (the dispatcher's `>> 0xE0`).
        let mut w = [0u8; 32];
        w[0..4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let shifted = shr256(&w, 224);
        assert_eq!(&shifted[28..], &[0xde, 0xad, 0xbe, 0xef]);
        assert!(shifted[..28].iter().all(|&b| b == 0));
    }

    // ── BOOTSTRAP: known-good shipped features must reproduce correctly ───────

    /// The const getter (`get() == 42`) — the golden gate, executed end-to-end.
    #[test]
    fn bootstrap_const_getter_returns_42() {
        let mut c = deploy_src(
            "facet C { function get() external view returns (uint256) { return 42; } }",
        );
        let ret = c.call(&calldata(sel("get()"), &[]), &CallEnv::default()).unwrap();
        assert_eq!(word_to_u64(&ret.as_slice().try_into().unwrap()), 42);
    }

    /// A bad selector hits the fallback `REVERT(0,0)`.
    #[test]
    fn bootstrap_unknown_selector_reverts() {
        let mut c = deploy_src(
            "facet C { function get() external view returns (uint256) { return 42; } }",
        );
        let err = c.call(&calldata([0x00, 0x00, 0x00, 0x00], &[]), &CallEnv::default()).unwrap_err();
        assert_eq!(err, ExecError::Revert(Vec::new()));
    }

    /// The Tally facet (`bump()` then `get()`): a storage write round-trips through
    /// SLOAD/ADD/SSTORE — the shipped storage-write MVP.
    #[test]
    fn bootstrap_tally_storage_write_round_trips() {
        const SRC: &str = "facet Tally { uint256 n; \
             function bump() external { n = n + 1; } \
             function get() external view returns (uint256) { return n; } }";
        let mut c = deploy_src(SRC);
        let env = CallEnv::default();
        let read = |c: &mut Contract| {
            word_to_u64(&c.call(&calldata(sel("get()"), &[]), &env).unwrap().as_slice().try_into().unwrap())
        };
        assert_eq!(read(&mut c), 0, "n starts at 0");
        c.call(&calldata(sel("bump()"), &[]), &env).unwrap();
        c.call(&calldata(sel("bump()"), &[]), &env).unwrap();
        c.call(&calldata(sel("bump()"), &[]), &env).unwrap();
        assert_eq!(read(&mut c), 3, "three bumps → 3");
    }

    /// The Ledger mapping (`add(amt)` writes `bal[msg.sender]`, `balanceOf(who)`
    /// reads it): the keccak mapping-slot derivation + CALLER + params, executed.
    #[test]
    fn bootstrap_ledger_mapping_per_caller() {
        const SRC: &str = "facet Ledger { mapping(address => uint256) bal; \
             function add(uint256 amt) external { bal[msg.sender] = bal[msg.sender] + amt; } \
             function balanceOf(address who) external view returns (uint256) { return bal[who]; } }";
        let mut c = deploy_src(SRC);
        let alice = CallEnv { caller: [0x11; 20], ..Default::default() };
        let bob = CallEnv { caller: [0x22; 20], ..Default::default() };
        c.call(&calldata(sel("add(uint256)"), &[word(10)]), &alice).unwrap();
        c.call(&calldata(sel("add(uint256)"), &[word(5)]), &alice).unwrap();
        c.call(&calldata(sel("add(uint256)"), &[word(99)]), &bob).unwrap();
        let bal = |c: &mut Contract, who: &[u8; 20]| {
            word_to_u64(
                &c.call(&calldata(sel("balanceOf(address)"), &[addr_word(who)]), &CallEnv::default())
                    .unwrap()
                    .as_slice()
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(bal(&mut c, &[0x11; 20]), 15, "alice = 10 + 5");
        assert_eq!(bal(&mut c, &[0x22; 20]), 99, "bob = 99");
        assert_eq!(bal(&mut c, &[0x33; 20]), 0, "an unseen caller = 0");
    }

    /// `require(n > 0, ...)` reverts on a failed guard, succeeds on a passing one —
    /// the shipped relational + require primitives, executed.
    #[test]
    fn bootstrap_require_guard_reverts_on_false() {
        const SRC: &str = "facet C { uint256 total; \
             function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"big\"); total = total + n; } \
             function get() external view returns (uint256) { return total; } }";
        let mut c = deploy_src(SRC);
        let env = CallEnv::default();
        // n = 0 → first require fails → revert.
        assert_eq!(
            c.call(&calldata(sel("incrementBy(uint256)"), &[word(0)]), &env).unwrap_err(),
            ExecError::Revert(Vec::new())
        );
        // n = 101 → second require fails → revert.
        assert_eq!(
            c.call(&calldata(sel("incrementBy(uint256)"), &[word(101)]), &env).unwrap_err(),
            ExecError::Revert(Vec::new())
        );
        // n = 7 → both pass → total becomes 7.
        c.call(&calldata(sel("incrementBy(uint256)"), &[word(7)]), &env).unwrap();
        let total =
            word_to_u64(&c.call(&calldata(sel("get()"), &[]), &env).unwrap().as_slice().try_into().unwrap());
        assert_eq!(total, 7);
    }

    /// The multiplicative tier + precedence (`x + x*x`) executes correctly —
    /// `poly(3) = 3 + 9 = 12` (NOT `(3+3)*3 = 18`), the shipped precedence proof.
    #[test]
    fn bootstrap_arithmetic_precedence() {
        const SRC: &str = "facet Math { \
             function poly(uint256 x) external pure returns (uint256) { return x + x * x; } \
             function fee(uint256 amount, uint256 rate) external pure returns (uint256) { return amount * rate / 10000; } }";
        let mut c = deploy_src(SRC);
        let env = CallEnv::default();
        let poly = word_to_u64(
            &c.call(&calldata(sel("poly(uint256)"), &[word(3)]), &env).unwrap().as_slice().try_into().unwrap(),
        );
        assert_eq!(poly, 12, "3 + 3*3 = 12 (precedence)");
        let fee = word_to_u64(
            &c.call(&calldata(sel("fee(uint256,uint256)"), &[word(1_000_000), word(250)]), &env)
                .unwrap()
                .as_slice()
                .try_into()
                .unwrap(),
        );
        assert_eq!(fee, 25_000, "1_000_000 * 250 / 10000");
    }

    /// The dynamic-array Stack (`push` / `xs[i]` / `xs.length` / `xs[i] = v`):
    /// push two, read length + elements, overwrite one — the shipped array MVP,
    /// executed at the canonical `keccak256(slot)+i` layout.
    #[test]
    fn bootstrap_dynamic_array_push_index_length() {
        const SRC: &str = "facet Stack { uint256 total; uint256[] xs; \
             function push(uint256 v) external { xs.push(v); total = total + 1; } \
             function set(uint256 i, uint256 v) external { xs[i] = v; } \
             function get(uint256 i) external view returns (uint256) { return xs[i]; } \
             function size() external view returns (uint256) { return xs.length; } }";
        let mut c = deploy_src(SRC);
        let env = CallEnv::default();
        let size = |c: &mut Contract| {
            word_to_u64(&c.call(&calldata(sel("size()"), &[]), &env).unwrap().as_slice().try_into().unwrap())
        };
        let get = |c: &mut Contract, i: u64| {
            word_to_u64(
                &c.call(&calldata(sel("get(uint256)"), &[word(i)]), &env).unwrap().as_slice().try_into().unwrap(),
            )
        };
        assert_eq!(size(&mut c), 0);
        c.call(&calldata(sel("push(uint256)"), &[word(11)]), &env).unwrap();
        c.call(&calldata(sel("push(uint256)"), &[word(22)]), &env).unwrap();
        assert_eq!(size(&mut c), 2, "two pushes → length 2");
        assert_eq!(get(&mut c, 0), 11);
        assert_eq!(get(&mut c, 1), 22);
        c.call(&calldata(sel("set(uint256,uint256)"), &[word(0), word(99)]), &env).unwrap();
        assert_eq!(get(&mut c, 0), 99, "set(0,99) overwrites in place");
    }

    /// An `emit` lowers to a real `LOGn`: topic0 = the event-sig keccak, the indexed
    /// arg becomes topic1, the data words land in the data region.
    #[test]
    fn bootstrap_emit_produces_a_log() {
        const SRC: &str = "facet C { event E(address indexed who, uint256 amt); \
             function f(uint256 n) external { emit E(msg.sender, n); } }";
        let art = compile(SRC).unwrap();
        let mut c = Contract::deploy(&art.init_code, &CallEnv::default()).unwrap();
        let env = CallEnv { caller: [0xAB; 20], ..Default::default() };
        let (_, logs) = c.call_logs(&calldata(sel("f(uint256)"), &[word(42)]), &env).unwrap();
        assert_eq!(logs.len(), 1, "one log");
        let log = &logs[0];
        assert_eq!(log.topics.len(), 2, "topic0 + indexed who");
        assert_eq!(log.topics[0], super::super::codegen::event_topic0("E(address,uint256)"));
        assert_eq!(log.topics[1], addr_word(&[0xAB; 20]), "topic1 is the caller");
        assert_eq!(log.data, word(42).to_vec(), "the data region holds n = 42");
    }

    // ── #37 NEW FEATURE: `.pop()` and `delete arr[i]` (diff-harness proven) ───

    /// The canonical `Stack` facet, EXTENDED with `pop()` and `clear(i)` (delete),
    /// built on the shipped length/[i]/push layout — the source under test.
    const POP_DELETE_SRC: &str = "facet Stack { uint256[] xs; \
         function push(uint256 v) external { xs.push(v); } \
         function pop() external { xs.pop(); } \
         function clear(uint256 i) external { delete xs[i]; } \
         function get(uint256 i) external view returns (uint256) { return xs[i]; } \
         function size() external view returns (uint256) { return xs.length; } }";

    /// `pop()` removes the last element: push 3, pop once → length 2, the popped
    /// element slot is zeroed, the remaining two persist. Then pop the rest → empty.
    #[test]
    fn pop_decrements_length_and_zeroes_the_removed_element() {
        let mut c = deploy_src(POP_DELETE_SRC);
        let env = CallEnv::default();
        let size = |c: &mut Contract| {
            word_to_u64(&c.call(&calldata(sel("size()"), &[]), &env).unwrap().as_slice().try_into().unwrap())
        };
        let get = |c: &mut Contract, i: u64| {
            word_to_u64(
                &c.call(&calldata(sel("get(uint256)"), &[word(i)]), &env).unwrap().as_slice().try_into().unwrap(),
            )
        };
        for v in [11u64, 22, 33] {
            c.call(&calldata(sel("push(uint256)"), &[word(v)]), &env).unwrap();
        }
        assert_eq!(size(&mut c), 3);

        c.call(&calldata(sel("pop()"), &[]), &env).unwrap();
        assert_eq!(size(&mut c), 2, "pop → length 2");
        assert_eq!(get(&mut c, 0), 11, "remaining elements persist");
        assert_eq!(get(&mut c, 1), 22);
        // The removed element's storage slot is now zero (re-pushing reuses it).
        c.call(&calldata(sel("push(uint256)"), &[word(44)]), &env).unwrap();
        assert_eq!(size(&mut c), 3, "re-push → length 3");
        assert_eq!(get(&mut c, 2), 44, "the reused slot holds the new value");

        // Pop everything down to empty.
        c.call(&calldata(sel("pop()"), &[]), &env).unwrap();
        c.call(&calldata(sel("pop()"), &[]), &env).unwrap();
        c.call(&calldata(sel("pop()"), &[]), &env).unwrap();
        assert_eq!(size(&mut c), 0, "popped to empty");
    }

    /// After a `pop()`, reading the popped index returns zero (the element slot was
    /// cleared) even though the storage slot is no longer "in bounds".
    #[test]
    fn popped_element_slot_reads_zero() {
        let mut c = deploy_src(POP_DELETE_SRC);
        let env = CallEnv::default();
        c.call(&calldata(sel("push(uint256)"), &[word(11)]), &env).unwrap();
        c.call(&calldata(sel("push(uint256)"), &[word(22)]), &env).unwrap();
        c.call(&calldata(sel("pop()"), &[]), &env).unwrap();
        // xs[1] is now out of bounds but its slot was zeroed by pop().
        let elem1 = word_to_u64(
            &c.call(&calldata(sel("get(uint256)"), &[word(1)]), &env).unwrap().as_slice().try_into().unwrap(),
        );
        assert_eq!(elem1, 0, "the popped element slot reads zero");
    }

    /// `delete xs[i]` zeroes the element IN PLACE, leaving the length unchanged and
    /// the neighboring elements intact (Solidity `delete arr[i]` semantics).
    #[test]
    fn delete_zeroes_element_in_place_keeping_length() {
        let mut c = deploy_src(POP_DELETE_SRC);
        let env = CallEnv::default();
        let size = |c: &mut Contract| {
            word_to_u64(&c.call(&calldata(sel("size()"), &[]), &env).unwrap().as_slice().try_into().unwrap())
        };
        let get = |c: &mut Contract, i: u64| {
            word_to_u64(
                &c.call(&calldata(sel("get(uint256)"), &[word(i)]), &env).unwrap().as_slice().try_into().unwrap(),
            )
        };
        for v in [11u64, 22, 33] {
            c.call(&calldata(sel("push(uint256)"), &[word(v)]), &env).unwrap();
        }
        c.call(&calldata(sel("clear(uint256)"), &[word(1)]), &env).unwrap();
        assert_eq!(size(&mut c), 3, "delete does NOT change the length");
        assert_eq!(get(&mut c, 0), 11, "neighbor before is intact");
        assert_eq!(get(&mut c, 1), 0, "the deleted element is zeroed");
        assert_eq!(get(&mut c, 2), 33, "neighbor after is intact");
    }

    /// `pop()` followed by `delete` interleave correctly over the shared layout.
    #[test]
    fn pop_and_delete_interleave() {
        let mut c = deploy_src(POP_DELETE_SRC);
        let env = CallEnv::default();
        let get = |c: &mut Contract, i: u64| {
            word_to_u64(
                &c.call(&calldata(sel("get(uint256)"), &[word(i)]), &env).unwrap().as_slice().try_into().unwrap(),
            )
        };
        let size = |c: &mut Contract| {
            word_to_u64(&c.call(&calldata(sel("size()"), &[]), &env).unwrap().as_slice().try_into().unwrap())
        };
        for v in [1u64, 2, 3, 4] {
            c.call(&calldata(sel("push(uint256)"), &[word(v)]), &env).unwrap();
        }
        c.call(&calldata(sel("clear(uint256)"), &[word(0)]), &env).unwrap(); // [0,2,3,4]
        c.call(&calldata(sel("pop()"), &[]), &env).unwrap(); // [0,2,3], len 3
        assert_eq!(size(&mut c), 3);
        assert_eq!(get(&mut c, 0), 0);
        assert_eq!(get(&mut c, 1), 2);
        assert_eq!(get(&mut c, 2), 3);
    }

    /// Direct storage-slot assertion: after `delete xs[1]`, the actual element
    /// storage slot `keccak256(slot)+1` is empty (zeroed), not merely reading zero
    /// through a getter. Independently derives the slot to cross-check the layout.
    #[test]
    fn delete_clears_the_actual_storage_slot() {
        let mut c = deploy_src(POP_DELETE_SRC);
        let env = CallEnv::default();
        for v in [11u64, 22, 33] {
            c.call(&calldata(sel("push(uint256)"), &[word(v)]), &env).unwrap();
        }
        // xs is the FIRST (and only) state var → base slot = keccak256("localharness.stack.storage.v1").
        let base = storage_base("stack");
        let elem1 = array_elem_slot(&base, 1);
        assert_ne!(c.sload(&elem1), [0u8; 32], "before delete the slot holds 22");
        c.call(&calldata(sel("clear(uint256)"), &[word(1)]), &env).unwrap();
        assert_eq!(c.sload(&elem1), [0u8; 32], "after delete the storage slot is zero");
    }

    /// The base storage slot of a facet (`keccak256("localharness.<name>.storage.v1")`)
    /// — the array's length lives here, mirroring `codegen::storage_base` (the FIRST
    /// state var, index 0). Test helper for the direct-slot assertions.
    fn storage_base(facet_name_lower: &str) -> Word {
        let preimage = format!("localharness.{facet_name_lower}.storage.v1");
        let mut w = [0u8; 32];
        w.copy_from_slice(&Keccak256::digest(preimage.as_bytes()));
        w
    }

    /// The element slot of a dynamic array: `keccak256(pad32(slot)) + index` — the
    /// canonical Solidity layout the codegen reproduces. Test helper.
    fn array_elem_slot(slot: &Word, index: u64) -> Word {
        let base: Word = Keccak256::digest(slot).into();
        super::add256(&base, &word(index))
    }

    /// A constant `returns (string)` getter returns the ABI string encoding (head
    /// offset 0x20, length, right-padded data) — decode it back to the literal.
    #[test]
    fn bootstrap_const_string_return_abi_encoding() {
        let mut c = deploy_src(
            "facet Meta { function name() external pure returns (string) { return \"claude\"; } }",
        );
        let ret = c.call(&calldata(sel("name()"), &[]), &CallEnv::default()).unwrap();
        // ABI: word0 = offset (0x20), word1 = length (6), word2 = data left-aligned.
        assert_eq!(word_to_u64(&ret[0..32].try_into().unwrap()), 0x20);
        let len = word_to_u64(&ret[32..64].try_into().unwrap()) as usize;
        assert_eq!(len, 6);
        assert_eq!(&ret[64..64 + len], b"claude");
    }
}
