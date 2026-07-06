//! Multi-chain EVM READ tools — thin re-export. The constructors were HOISTED
//! to `crate::evm_tools` (native + wasm, feature `wallet`) so the headless CLI
//! `call` registers the same set (fleet F2: a tool-free headless turn
//! fabricated from-memory addresses). Keep this a re-export; add new EVM read
//! tools THERE.

pub(crate) use crate::evm_tools::{
    evm_balance_tool, evm_call_tool, evm_chains_tool, resolve_ens_tool,
};
