// localharness credit proxy — best-effort in-isolate sliding-window rate
// limiter (shared by api/notify.ts + api/broadcast.ts).
//
// WHY IT EXISTS. Every proxy route is metered in on-chain $LH, so abuse is
// never FREE — but the meter alone has no notion of RATE: a funded caller can
// buzz a phone (notify, ~0.01 $LH/push) or fan out broadcasts in a tight loop
// and the only thing that stops them is their balance running out. For the
// ABUSE-VISIBLE routes (pushes land on a human's phone) we add a cheap
// requests-per-window cap on top. Model calls (gemini.ts) are deliberately
// NOT rate-limited — they're the product, and the meter prices them.
//
// ⚠️ LIMITATION — PER-ISOLATE, NOT GLOBAL. Edge functions share no state
// across isolates; this Map lives in ONE warm isolate. Vercel may run several
// isolates per region (and several regions), so the effective cap is
// "N per window PER ISOLATE". It reliably defeats the common abuse shape (one
// caller hammering one warm isolate in a tight loop); a determined attacker
// spreading requests across isolates dilutes it. That is ACCEPTED: a hard
// global limit needs shared state (KV/Redis) and the platform's rule is no
// off-chain infra beyond this proxy — the on-chain meter debit remains the
// durable, global cost ceiling. This is the cheap first line, not the wall.
//
// ⚠️ PRE-AUTH BY DESIGN. Callers check the window on the CLAIMED address from
// the auth token BEFORE signature verification (rejecting a flood must not
// cost a curve recovery per request). Safe because the limiter gates nothing
// of value: worst case an attacker spoofing someone's address burns that
// address's rate WINDOW in this isolate (a minutes-long nuisance), never
// their FUNDS — the meter debit only ever happens after real signature
// verification, downstream of this check.

/**
 * A fixed-capacity sliding window: at most `limit` hits per `windowMs` per
 * key. `hit()` both checks AND records (a passing call consumes a slot).
 */
export class SlidingWindow {
  private readonly hits = new Map<string, number[]>();

  constructor(
    private readonly limit: number,
    private readonly windowMs: number,
  ) {}

  /**
   * Try to consume a slot for `key`. Returns 0 when allowed (the hit is
   * recorded), else the number of seconds until the oldest in-window hit
   * expires (use as `retryAfterSeconds`; the hit is NOT recorded).
   */
  hit(key: string, now: number = Date.now()): number {
    const cutoff = now - this.windowMs;
    let stamps = this.hits.get(key);
    if (stamps) {
      while (stamps.length > 0 && stamps[0] <= cutoff) stamps.shift();
      if (stamps.length === 0) this.hits.delete(key);
    }
    if (stamps && stamps.length >= this.limit) {
      return Math.max(1, Math.ceil((stamps[0] + this.windowMs - now) / 1000));
    }
    if (!stamps || stamps.length === 0) {
      stamps = [];
      this.hits.set(key, stamps);
    }
    stamps.push(now);
    this.sweep(cutoff);
    return 0;
  }

  /** Bound isolate memory: when the key table grows past a small cap, drop
   * every key whose hits have all aged out (an attacker rotating keys can
   * only ever pin live-window entries). */
  private sweep(cutoff: number): void {
    if (this.hits.size <= 4096) return;
    for (const [key, stamps] of this.hits) {
      while (stamps.length > 0 && stamps[0] <= cutoff) stamps.shift();
      if (stamps.length === 0) this.hits.delete(key);
    }
  }
}

/**
 * The CLAIMED (unverified!) lowercase 0x address from a proxy auth token
 * (`<address>:<timestamp>:<signature>`), or '' when the token doesn't even
 * carry an address-shaped first segment. For rate-limit keying ONLY — never
 * authorize anything off this value (see the PRE-AUTH note above).
 */
export function claimedAddress(token: string): string {
  const first = token.split(':')[0] ?? '';
  return /^0x[0-9a-fA-F]{40}$/.test(first) ? first.toLowerCase() : '';
}
