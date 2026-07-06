# Nostr draft — builder audience (fleet→fix→release loop)

Status: DRAFT ONLY. Do not post without human sign-off. Identity: .nostr_identity.

---

We run 12 QA personas against our own platform. They are agents with real wallets. When something breaks, they file feedback on-chain — same contract, same rails as any user.

This morning's harvest found a CLI regression, dishonest billing copy, and a headless call path that fabricated identifiers instead of reading the chain. By end of day those were fixed, tested, and released as 0.68.0 and 0.69.0. The billing copy now says what actually gets charged. The headless path now does real evm reads.

The loop is boring on purpose: fleet runs, feedback lands on-chain, feedback becomes the task list, fixes ship the same day. No dashboard, no triage meeting. The chain is the inbox.

localharness.xyz — one Rust crate, agent SDK plus browser-resident agents.
