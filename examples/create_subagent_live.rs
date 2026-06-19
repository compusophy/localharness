//! E2E proof: create a real agent subdomain on live Tempo Moderato using
//! the SAME SDK code paths the browser app's `create_subdomain` tool uses
//! — `wallet` (fresh identity) + `registry::claim_and_maybe_set_main_
//! sponsored` (sponsored Tempo 0x76 tx; the user holds zero of anything).
//! Then it reads the chain back to prove the registration landed.
//!
//! Run: `cargo run --example create_subagent_live --features wallet`
//! Optional first arg overrides the auto-derived name.
//!
//! This WRITES to the live testnet registry (diamond
//! 0x6c31c0…Da30c) and spends the embedded sponsor's AlphaUSD. The name
//! is releasable later via ReleaseFacet `releaseName`.

use localharness::registry;
use localharness::wallet;

// Embedded testnet sponsor — identical to `src/app/sponsor.rs`. Pays the
// AlphaUSD fees so the fresh agent identity needs no balance.
const SPONSOR_KEY: &str = "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";

#[tokio::main]
async fn main() {
    // 1. A brand-new on-chain identity for the subagent.
    let agent = wallet::generate();
    let addr = agent.address_hex();

    // 2. A unique, registry-valid (a-z0-9) name derived from the address.
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("claude{}", &addr[2..8]));

    println!("== create-subagent E2E (live Moderato) ==");
    println!("registry : {}", registry::REGISTRY_ADDRESS());
    println!("identity : {addr}  (freshly generated, holds nothing)");
    println!("name     : {name}.localharness.xyz");

    let sponsor = wallet::from_private_key_hex(SPONSOR_KEY).expect("sponsor key parse");

    // 2b. PERSIST the identity BEFORE writing on-chain, so the key is never
    // lost even if registration fails — this is what makes the subagent
    // *controllable* across sessions rather than a write-once orphan.
    let identity_line = format!(
        "name={name}\naddress={addr}\nprivate_key={}\n",
        agent.private_key_hex
    );
    if let Err(e) = std::fs::write("my-agent-identity.txt", &identity_line) {
        eprintln!("!! could not persist identity, aborting before on-chain write: {e}");
        std::process::exit(1);
    }
    println!("identity persisted -> my-agent-identity.txt");
    println!("PRIVATE KEY (testnet, store securely): {}", agent.private_key_hex);

    // 3. Pre-check the name is free (so a collision reads as that, not a fail).
    match registry::owner_of_name(&name).await {
        Ok(Some(o)) => {
            eprintln!("!! name already registered to {o} — pass a different name as arg1");
            std::process::exit(2);
        }
        Ok(None) => println!("name is free — registering…"),
        Err(e) => {
            eprintln!("!! RPC error checking name: {e}");
            std::process::exit(1);
        }
    }

    // 4. Sponsored register — the exact path `create_subdomain` runs.
    let tx_hash = match registry::claim_and_maybe_set_main_sponsored(
        &agent.signer,
        &sponsor,
        &name,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(h) => h,
        Err(e) => {
            eprintln!("!! registration failed: {e}");
            std::process::exit(1);
        }
    };
    println!("registered — tx {tx_hash}");

    // 5. Read the chain back: the new name must resolve to the new identity.
    match registry::owner_of_name(&name).await {
        Ok(Some(owner)) if owner.eq_ignore_ascii_case(&addr) => {
            let id = registry::id_of_name(&name).await.unwrap_or(0);
            println!("VERIFIED on-chain:");
            println!("  owner   : {owner}");
            println!("  tokenId : {id}");
            println!("  url     : https://{name}.localharness.xyz/");
            println!("== PASS: subagent created and verified ==");
        }
        other => {
            eprintln!("!! verification mismatch: {other:?}");
            std::process::exit(1);
        }
    }
}
