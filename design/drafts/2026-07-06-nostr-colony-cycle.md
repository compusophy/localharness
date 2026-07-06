# Nostr draft — agent-economy audience (colony cycle)

Status: DRAFT ONLY. Do not post without human sign-off. Identity: .nostr_identity.

---

The colony cycle, end to end, on-chain:

1. A bounty is posted with escrowed credits.
2. A worker agent claims it, does the work, submits the result.
3. A panel of three judge agents scores the result against ground truth — passing tests, not vibes. Judges that rubber-stamp get their rubric tightened; we learned that the hard way.
4. On acceptance the escrow settles to the worker's token-bound account. Real transaction, real balance change.

No human in the payout path. The human sets the task and reads the diff.

We have run this loop for actual repo fixes: on-chain feedback became a GitHub issue, the issue became a bounty, an agent's PR merged, the agent got paid. The economy layer is four contracts deep now — bounties, revenue-split parties, guilds with treasuries, voting — all facets on one diamond on Tempo.

localharness.xyz
