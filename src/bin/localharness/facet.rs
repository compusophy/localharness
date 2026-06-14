// `localharness facet` — author + ship on-chain facets with the in-crate
// SolidityLite compiler. The harness side of the self-modifying-platform
// keystone: an agent writes a Solidity-subset facet in source and deploys it
// with ONE command (compile → sponsored CREATE), no toolchain, no solc.
//
//   localharness facet deploy [--as <me>] <name> <src.sol>
//
// Prints the deployed facet address; cut it into a diamond you own with
// `diamondCut(FacetCut[])` (registry::encode_diamond_cut).

#[allow(unused_imports)]
use crate::*;
use crate::util::load_signer_and_sponsor;
use localharness::registry;
use localharness::soliditylite::compile;

pub(crate) async fn facet(caller: Option<&str>, args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("deploy") => facet_deploy(caller, &args[1..]).await,
        _ => {
            eprintln!("usage: localharness facet deploy [--as <me>] <name> <src.sol>");
            2
        }
    }
}

async fn facet_deploy(caller: Option<&str>, rest: &[String]) -> i32 {
    let (name, path) = match (rest.first(), rest.get(1)) {
        (Some(n), Some(p)) => (n.as_str(), p.as_str()),
        _ => {
            eprintln!("usage: localharness facet deploy [--as <me>] <name> <src.sol>");
            return 2;
        }
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("read {path}: {e}");
            return 1;
        }
    };
    // Compile FIRST — a source error costs nothing on-chain.
    let art = match compile(&src) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("compile error: {e:?}");
            return 1;
        }
    };
    // Length-scaled gas (sponsor pays gas USED, so headroom is free): base +
    // the code-deposit-dominated cost of the runtime, at Tempo's high rates.
    let gas = 2_000_000 + art.init_code.len() as u128 * 6_000;
    println!(
        "compiled '{name}': {}-byte runtime → deploying via sponsored CREATE …",
        art.runtime.len()
    );
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::create_sponsored(&signer, &sponsor, art.init_code, registry::ALPHA_USD_ADDRESS, gas).await {
        Ok(addr) => {
            println!("✓ deployed '{name}' facet at {addr}");
            println!("  next: cut it into a diamond you own — diamondCut(FacetCut[]) via registry::encode_diamond_cut");
            0
        }
        Err(e) => {
            eprintln!("deploy failed: {e}");
            1
        }
    }
}
