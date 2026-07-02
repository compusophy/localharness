// `localharness facet` — author + ship + wire on-chain facets with the in-crate
// SolidityLite compiler. The harness side of the self-modifying-platform
// keystone: an agent writes a Solidity-subset facet in source and runs the whole
// flow from the CLI, no toolchain, no solc.
//
//   localharness facet deploy  [--as <me>] <name> <src.sol>       compile + deploy a facet
//   localharness facet diamond [--as <me>]                        genesis a diamond you OWN
//   localharness facet cut     [--as <me>] <diamond> <facet> <src.sol>   cut a facet into your diamond
//
// Typical flow: `facet diamond` → your diamond addr; `facet deploy x x.sol` →
// facet addr; `facet cut <diamond> <facet> x.sol` → it's live behind the diamond.

use crate::util::{load_signer, load_sponsor};
use localharness::cut_guard;
use localharness::registry::{self, FacetCut};
use localharness::soliditylite::compile;
use localharness::tempo_tx::TempoCall;
use localharness::wallet;

// Prod stateless core facets to seed a child diamond with (reused as code via
// delegatecall in the child's own storage). Loupe/Ownership are queried from the
// canonical diamond's loupe; the cut entry point is the GUARDED cut facet (the
// §7 on-chain twin of `cut_guard`) so a child diamond is safe-by-construction —
// its owner can't `diamondCut` a reserved selector or an init delegatecall even
// by signing a raw tx (`GuardedDiamondCutFacet.sol`, deployed 2026-06-14).
const GUARDED_CUT_FACET: &str = "0xa4c8a030607090e0C8602311F104471381E94eb1";
const LOUPE_FACET: &str = "0x28577026cDEeAb9b9E723666e16c530b94c9EED3";
const OWN_FACET: &str = "0x9D157FaAEB76956986aAc1b96afCE9Efe0D1CEc4";

const USAGE: &str = "usage:\n  localharness facet deploy  [--as <me>] <name> <src.sol>\n  localharness facet diamond [--as <me>]\n  localharness facet cut     [--as <me>] <diamond-addr> <facet-addr> <src.sol>";

pub(crate) async fn facet(caller: Option<&str>, args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("deploy") => facet_deploy(caller, &args[1..]).await,
        Some("diamond") => facet_diamond(caller).await,
        Some("cut") => facet_cut(caller, &args[1..]).await,
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

async fn facet_deploy(caller: Option<&str>, rest: &[String]) -> i32 {
    let (name, path) = match (rest.first(), rest.get(1)) {
        (Some(n), Some(p)) => (n.as_str(), p.as_str()),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let art = match read_and_compile(path) {
        Ok(a) => a,
        Err(c) => return c,
    };
    let gas = 2_000_000 + art.init_code.len() as u128 * 6_000;
    println!("compiled '{name}': {}-byte runtime, {} selector(s) → deploying via sponsored CREATE …", art.runtime.len(), art.selectors.len());
    let signer = match load_signer(caller) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(c) => return c,
    };
    match registry::create_sponsored(&signer, &sponsor, art.init_code, registry::ALPHA_USD_ADDRESS(), gas).await {
        Ok(addr) => {
            println!("✓ deployed '{name}' facet at {addr}");
            for s in &art.selectors {
                println!("    selector 0x{}", hex(s));
            }
            println!("  cut it in:  localharness facet cut <your-diamond> {addr} {path}");
            0
        }
        Err(e) => {
            eprintln!("deploy failed: {e}");
            1
        }
    }
}

async fn facet_diamond(caller: Option<&str>) -> i32 {
    // Diamond creation bytecode: PREFER a local forge build (in-repo dev with an
    // edited Diamond.sol), else fall back to the embedded bytecode so an INSTALLED
    // CLI (no `contracts/out`) can still genesis a diamond anywhere.
    let bytecode = std::fs::read_to_string("contracts/out/Diamond.sol/Diamond.json")
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|j| j["bytecode"]["object"].as_str().map(str::to_string))
        .unwrap_or_else(|| crate::diamond_bytecode::DIAMOND_INIT_HEX.to_string());
    let mut init = match hex_decode(bytecode.trim_start_matches("0x")) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bad Diamond bytecode: {e}");
            return 1;
        }
    };
    let signer = match load_signer(caller) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let owner = wallet::address(&signer);
    let gcuts = vec![
        FacetCut { facet: addr20(GUARDED_CUT_FACET), action: 0, selectors: vec![[0x1f, 0x93, 0x1c, 0x1c]] },
        FacetCut { facet: addr20(LOUPE_FACET), action: 0, selectors: vec![[0x7a,0x0e,0xd6,0x27],[0xad,0xfc,0xa1,0x5e],[0x52,0xef,0x6b,0x2c],[0xcd,0xff,0xac,0xc6],[0x01,0xff,0xc9,0xa7]] },
        FacetCut { facet: addr20(OWN_FACET), action: 0, selectors: vec![[0xf2,0xfd,0xe3,0x8b],[0x8d,0xa5,0xcb,0x5b]] },
    ];
    init.extend_from_slice(&registry::encode_diamond_constructor_args(&owner, &gcuts));
    println!("genesis-ing a child diamond owned by you (0x{}), guarded cut facet, via sponsored CREATE …", hex(&owner));
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(c) => return c,
    };
    match registry::create_sponsored(&signer, &sponsor, init, registry::ALPHA_USD_ADDRESS(), 25_000_000).await {
        Ok(addr) => {
            println!("✓ your diamond: {addr}");
            println!("  cut facets into it:  localharness facet cut {addr} <facet-addr> <src.sol>");
            0
        }
        Err(e) => {
            eprintln!("genesis failed: {e}");
            1
        }
    }
}

async fn facet_cut(caller: Option<&str>, rest: &[String]) -> i32 {
    let (diamond, facet_addr, path) = match (rest.first(), rest.get(1), rest.get(2)) {
        (Some(d), Some(f), Some(p)) => (d.as_str(), f.as_str(), p.as_str()),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let art = match read_and_compile(path) {
        Ok(a) => a,
        Err(c) => return c,
    };
    // §7 safety pre-flight: refuse reserved/clashing/duplicate selectors and a
    // non-zero `_init` BEFORE spending cut gas (a clash reverts the whole tx) or
    // letting a facet seize the diamond. Clash detection queries the diamond's
    // loupe for each new selector; reserved + dup checks are pure.
    let mut present: Vec<[u8; 4]> = Vec::new();
    for s in &art.selectors {
        if let Ok(Some(_)) = registry::facet_address_of(diamond, *s).await {
            present.push(*s);
        }
    }
    if let Err(reasons) = cut_guard::check_cut(&art.selectors, &present, true) {
        eprintln!("✗ cut rejected by safety lint ({} issue(s)):", reasons.len());
        for r in &reasons {
            eprintln!("  - {r}");
        }
        return 1;
    }
    let n = art.selectors.len();
    let cut = FacetCut { facet: addr20(facet_addr), action: 0, selectors: art.selectors };
    let calldata = registry::encode_diamond_cut(&[cut], &[0u8; 20], &[]);
    let signer = match load_signer(caller) {
        Ok(p) => p,
        Err(c) => return c,
    };
    println!("cutting {n} selector(s) of {facet_addr} into {diamond} (safety-lint OK; diamondCut, sponsored) …");
    match registry::sponsored_batch(
        &signer,
        vec![TempoCall { to: addr20(diamond), value_wei: 0, input: calldata }],
        12_000_000,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ cut into {diamond} (tx {tx}) — the facet's functions are now live behind the diamond");
            0
        }
        Err(e) => {
            eprintln!("cut failed: {e}");
            1
        }
    }
}

fn read_and_compile(path: &str) -> Result<localharness::soliditylite::CompiledArtifact, i32> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("read {path}: {e}");
        1
    })?;
    compile(&src).map_err(|e| {
        // Render the agent-friendly diagnostic (LHxxxx label + `line N, col M` +
        // a caret-marked snippet), matching `compile_rustlite` / `publish` — NOT
        // the raw `Debug` struct with byte offsets (cryptic + unactionable).
        eprintln!("compile error:\n{}", e.render(&src));
        1
    })
}

/// Parse a 0x-hex 20-byte address; an invalid address becomes the zero address
/// (the on-chain call then fails loudly rather than the CLI panicking).
fn addr20(h: &str) -> [u8; 20] {
    let mut out = [0u8; 20];
    if let Ok(b) = hex_decode(h.trim().trim_start_matches("0x")) {
        if b.len() == 20 {
            out.copy_from_slice(&b);
        }
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
