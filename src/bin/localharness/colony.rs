use crate::{bounty_work_ref, bytes_to_hex_str, collect_flags, credits, ensure_wallet_covers, fmt_lh, identity_key_files, load_signer_and_sponsor, parse_ttl, registry, report_call_error, resolve_caller_key, resolve_caller_label, resolve_key_read_path, resolve_own_token_id, run_agent_turn, wallet, INVITE_DEFAULT_TTL_SECS, KEY_SUFFIX};

// ---- colony (the agent economy's first autonomous cycle) ------------------
//
// `colony run` composes the bounty lifecycle + a headless `call` + a headless
// JUDGE PANEL into ONE self-driving turn of the demand flywheel: the platform
// (the caller) POSTS real work as an escrowed bounty, a WORKER agent claims it,
// the worker's on-chain persona DOES the work (an LLM turn via the credit proxy),
// the worker submits the result, a NEUTRAL JUDGE PANEL (median of N, default 3)
// scores it 1-5 for genuine + accurate task-fit, the caller accepts — settling
// the reward to the worker's token-bound account — and finally ATTESTS the
// PANEL'S MEDIAN rating on-chain (NOT a hardcoded 5★), so the worker's reputation
// reflects judged quality and rewards no hallucination. The panel EXCLUDES the
// worker AND the caller (neutrality), which matters because that reputation now
// DRIVES the PICK step. No human orchestrates the steps. The result + judge TEXT
// are LLM turns (they vary); the CYCLE mechanics
// (post→claim→submit→accept→payout→attest) are deterministic. Every on-chain step
// reuses the SAME helpers as the `bounty` subcommands (`post_bounty_sponsored` /
// `claim_bounty_sponsored` / `submit_result_sponsored` / `accept_result_sponsored`
// / `attest_sponsored`) and the work + each judge reuse the SAME headless turn as
// `call` (`run_agent_turn`), so it adds no new on-chain surface.

pub(crate) const COLONY_USAGE: &str = "\
usage: localharness colony run [--as <me>] <task> --reward <lh> [--worker <agent>] [--judges <N>] [--judge <agent>] [--min-accept-rating <N>] [--ttl <dur>]
  Run ONE autonomous agent-economy cycle end-to-end:
    1. the caller (--as, default your sole identity) POSTS <task> as a bounty escrowing <reward> $LH
    2. a WORKER is picked: --worker <agent>, else the reputation-aware top discover() match for <task>
    3. the worker CLAIMS the bounty (reward bound to the worker's token-bound account)
    4. the worker's on-chain persona DOES the work via a headless `call`
    5. the worker SUBMITS the produced result
    6. a NEUTRAL JUDGE PANEL scores the result 1-5 for genuine + accurate task-fit (catches
       hallucinations); the worker's rating is the MEDIAN of the panel
    7. PAYMENT GATE — IFF the median >= --min-accept-rating the caller ACCEPTS → the escrowed
       $LH settles to the worker's TBA; otherwise the result is REJECTED (NOT paid — the escrow
       stays locked and is reclaimable via `bounty reclaim` after the ttl)
    8. the caller ATTESTS to the worker (the panel's MEDIAN rating, workRef = the bounty id) →
       reputation — ALWAYS, accept OR reject (a rejected low rating must still hit the chain)
  --reward <lh>          the $LH reward to escrow (e.g. 0.02)            [required]
  --worker <agent>       the worker subdomain (its key must be local);
                         omit to auto-pick the best discover() match
  --judges <N>           size of the auto-selected neutral judge panel (default 3); N DISTINCT
                         local agents EXCLUDING the worker AND the caller are chosen, the median
                         of their ratings is attested. Fewer than N → uses what's available (min 1)
  --judge <agent>        force a SINGLE named judge (a panel of exactly that one agent; its key
                         must be local); overrides --judges
  --min-accept-rating N  PAYMENT GATE (1..5, default 2): the colony accepts + pays IFF the panel
                         median is >= N. A median below N is REJECTED — the worker is NOT paid and
                         the escrow stays locked (reclaim it after the ttl). Default 2 ⇒ a median
                         of 1 (clear failure / hallucination) is rejected; 2-5 are paid
  --ttl <dur>            bounty expiry (1h/7d/30d, 1h…90d, default 7d)
  The worker MUST be a fleet/owned agent whose key is in your keys dir
  (it signs its own claim + submit). The neutral panel makes the reputation signal
  TRUSTWORTHY — which matters because reputation now DRIVES the PICK step. On any
  step failure the bounty id + the CORRECT recovery command is printed (`bounty
  cancel` while OPEN, else `bounty reclaim` after the ttl) — never a silent
  half-state. The colony is economically rational: it pays ONLY for work the
  neutral panel rates at/above the bar; a sub-bar result is rejected (no payment,
  escrow recoverable) yet STILL attested so reputation reflects it. If no neutral
  agent exists the caller acts as a lone fallback judge; if ALL judge turns fail
  the median defaults to a neutral 3★.";

/// Rustlite item keywords a top-level repro line plausibly starts with (compiled
/// as-is), and statement keywords that must be WRAPPED in `fn main` to form a valid
/// module. Used by [`is_rustlite_code`] + [`compile_repro`].
const RL_ITEM_STARTS: &[&str] = &["fn ", "const ", "static ", "struct ", "enum ", "use "];
const RL_STMT_STARTS: &[&str] = &["let "];

/// `true` if a trimmed line looks like rustlite code: starts with an item/statement
/// keyword AND carries code punctuation (`=`/`(`/`{`). The punctuation guard keeps
/// prose like "use the config value" from being mistaken for a repro.
fn is_rustlite_code(s: &str) -> bool {
    let t = s.trim();
    // A leading attribute (e.g. `#[no_mangle]`) is unambiguously code; otherwise
    // require an item/statement keyword at the start.
    let starts_code =
        t.starts_with("#[") || RL_ITEM_STARTS.iter().chain(RL_STMT_STARTS).any(|k| t.starts_with(k));
    starts_code && (t.contains('=') || t.contains('(') || t.contains('{'))
}

/// Pure: pull the first plausible rustlite repro out of a free-form worker
/// `result`, so the judge can be handed GROUND-TRUTH compile evidence instead of
/// guessing. Looks (in order) for a ```-fenced code block, a single-backtick span,
/// then a BARE line — each recognized by [`is_rustlite_code`]. Returns `None` when
/// nothing code-like is present (most non-code tasks) → no evidence injected, judge
/// unchanged. The bare-line scan is what catches an UNQUOTED repro like
/// `const X: i32 = <huge>;` (seen dogfooding — the worker didn't wrap it, so the
/// first backtick-only version of this extractor silently found nothing).
pub(crate) fn extract_rustlite_snippet(result: &str) -> Option<String> {
    // 1. Triple-backtick fenced block (odd segments are inside fences).
    if result.contains("```") {
        for seg in result.split("```").skip(1).step_by(2) {
            // Drop a short leading language tag line (```rust / ```rl) if present.
            let body = match seg.split_once('\n') {
                Some((first, rest)) if first.trim().len() <= 8 && !is_rustlite_code(first) => rest,
                _ => seg,
            };
            if body.lines().any(is_rustlite_code) {
                return Some(body.trim().to_string());
            }
        }
    }
    // 2. First single-backtick span that looks like code.
    let mut i = 0;
    while let Some(rel) = result[i..].find('`') {
        let start = i + rel + 1;
        let Some(end_rel) = result[start..].find('`') else { break };
        let span = &result[start..start + end_rel];
        if is_rustlite_code(span) {
            return Some(span.trim().to_string());
        }
        i = start + end_rel + 1;
    }
    // 3. First BARE (unquoted) line that looks like code.
    result.lines().find(|l| is_rustlite_code(l)).map(|l| l.trim().to_string())
}

/// The three ground-truth outcomes of compiling a repro — the axis a code-bug
/// judge actually needs: does it CRASH the compiler, get REJECTED cleanly, or
/// BUILD?
pub(crate) enum ReproOutcome {
    Compiles(usize),
    CleanError(String),
    Crash,
}

/// Serializes the process-global panic-hook swap in [`compile_repro`] (cargo runs
/// tests in parallel, so the swap must not race).
static REPRO_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Compile a repro with the real rustlite compiler. A bare statement (`let …`) is
/// wrapped in `fn main` so it's a valid module; a top-level item compiles as-is. A
/// panic is CAUGHT so a pathological repro can never bail the colony cycle — and a
/// genuine panic is itself the finding ([`ReproOutcome::Crash`]).
pub(crate) fn compile_repro(snippet: &str) -> ReproOutcome {
    let t = snippet.trim();
    let is_item = RL_ITEM_STARTS.iter().any(|k| t.starts_with(k));
    // rustlite needs a `frame`/`render` cartridge entry (else LH0302), so a VALID
    // repro would otherwise read as a false "rejection". Give every snippet an
    // entry: keep it as-is if it already defines one; append a trivial entry to a
    // top-level item; wrap a bare statement inside the entry body.
    let has_entry = t.contains("fn frame") || t.contains("fn render");
    let src = if has_entry {
        t.to_string()
    } else if is_item {
        format!("{t}\n#[no_mangle] fn frame(t: i32) -> i32 {{ t }}")
    } else {
        format!("#[no_mangle] fn frame(t: i32) -> i32 {{ {t} t }}")
    };
    let _guard = REPRO_HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // silence the default print; a panic IS the finding.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        localharness::rustlite::compile(&src)
    }));
    std::panic::set_hook(prev);
    match r {
        Ok(Ok(b)) => ReproOutcome::Compiles(b.len()),
        Ok(Err(e)) => {
            let code = e.code.map(|c| format!("LH{c:04}")).unwrap_or_else(|| "LH????".into());
            ReproOutcome::CleanError(format!("{code}: \"{}\"", e.message))
        }
        Err(_) => ReproOutcome::Crash,
    }
}

/// Compile the worker result's embedded rustlite repro (if any) and format ONE
/// line of ground-truth evidence for the judge — deterministic. `None` when
/// there's no code-like snippet. Distinguishes a REAL crash (which SUPPORTS a
/// crash-claim) from a clean coded rejection (which REFUTES one) from a clean
/// build, so the judge can score the accuracy axis instead of guessing.
pub(crate) fn rustlite_compile_evidence(result: &str) -> Option<String> {
    let snippet = extract_rustlite_snippet(result)?;
    Some(match compile_repro(&snippet) {
        ReproOutcome::Compiles(n) => format!(
            "The repro `{snippet}` COMPILES cleanly ({n} bytes) with the real rustlite \
             compiler — no crash, no miscompile. A worker claiming it crashes or miscompiles is \
             inaccurate."
        ),
        ReproOutcome::CleanError(detail) => format!(
            "The repro `{snippet}` is REJECTED by the real rustlite compiler with a clean coded \
             error {detail}. A clean coded error is the compiler's INTENDED handling of \
             unsupported/invalid input — it is NOT a crash or a miscompile. A worker claiming a \
             crash, a silent miscompile, or a mechanism this error contradicts is inaccurate."
        ),
        ReproOutcome::Crash => format!(
            "The repro `{snippet}` actually CRASHES (panics) the real rustlite compiler — a \
             GENUINE crash bug. A worker claiming a crash here is ACCURATE."
        ),
    })
}

/// Build the impartial-judge prompt for the [6/8] JUDGE step. The judge scores
/// the worker's `result` against the `task` on a 1-5 scale, explicitly checking
/// for ACCURACY/hallucination (with the serverless-localharness context baked in
/// so a "binds a port / control API" style fabrication scores low). When the
/// result embeds a rustlite repro, GROUND-TRUTH compile evidence is injected so
/// the judge stops rubber-stamping unverifiable compiler lore (the false-5★ seen
/// dogfooding). The reply's first line MUST be a single 1-5 digit; the rest is
/// rationale.
pub(crate) fn colony_judge_prompt(task: &str, result: &str) -> String {
    let evidence = rustlite_compile_evidence(result)
        .map(|e| {
            format!(
                "\nGROUND-TRUTH COMPILE EVIDENCE (deterministic — from actually running the real \
                 rustlite compiler on the repro; TRUST THIS over your own intuition about \
                 compilers):\n{e}\n"
            )
        })
        .unwrap_or_default();
    format!(
        "You are an impartial judge scoring a bounty result.\n\
         TASK: {task}\n\
         WORKER RESULT: {result}\n{evidence}\n\
         Score 1-5 whether the result genuinely AND ACCURATELY addresses the task \
         (5 = excellent, specific, correct; 1 = irrelevant, wrong, or HALLUCINATED). \
         IMPORTANT context for accuracy-checking: localharness is SERVERLESS — it runs \
         on the Tempo chain + the browser + a Vercel edge proxy; there is NO local \
         server/daemon/control-API/port binding. A result that claims to fix or find \
         such a thing is HALLUCINATED and scores low. When COMPILE EVIDENCE is present \
         above, it is ground truth: a finding whose claimed mechanism the evidence \
         contradicts is inaccurate and scores low.\n\n\
         Output ONLY a single digit 1-5 on the first line, then one short line of rationale."
    )
}

/// Parse a judge's reply into `(rating, rationale)`. The rating is the FIRST
/// `1..=5` digit on the FIRST NON-EMPTY LINE (the prompt pins the score to line
/// 1; a chatty model may prepend a word, but a number further down in the
/// rationale — a year, a count — must NOT be mistaken for the score);
/// unparseable → a neutral default of 3. The rationale is the first non-empty
/// line that is not just the bare rating digit. Pure + testable.
pub(crate) fn parse_judge_rating(reply: &str) -> (u8, String) {
    let rating = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .and_then(|line| {
            line.chars()
                .find_map(|c| c.to_digit(10).filter(|d| (1..=5).contains(d)))
        })
        .map(|d| d as u8)
        .unwrap_or(3);
    let rationale = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && l.trim_matches(|c: char| !c.is_alphanumeric()).len() > 1)
        .unwrap_or("")
        .to_string();
    (rating, rationale)
}

/// Aggregate a NEUTRAL JUDGE PANEL's per-judge ratings into a single MEDIAN
/// rating (the robust, outlier-resistant centre — one rogue judge can't swing
/// it the way a mean would). Pure + testable.
///
/// Rule: sort the ratings ascending; **odd N** → the middle element; **even N**
/// → the LOWER-MIDDLE element (`[n/2 - 1]`) — a deliberately conservative tie
/// break so a split panel never rounds reputation UP. An EMPTY slice → a neutral
/// `3` (the same default the colony uses when every judge turn fails, so the
/// cycle completes with an honest, non-inflated rating). The result is always in
/// `1..=5` given `1..=5` inputs (median of in-range values is in range).
pub(crate) fn median_rating(ratings: &[u8]) -> u8 {
    if ratings.is_empty() {
        return 3;
    }
    let mut sorted = ratings.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    // Odd: the true middle. Even: the lower-middle (conservative — don't inflate).
    let idx = if n % 2 == 1 { n / 2 } else { n / 2 - 1 };
    sorted[idx]
}

/// PAYMENT GATE for `colony run`: should the caller ACCEPT (pay) given the panel
/// `median` and the `--min-accept-rating` threshold? Pure + testable — the colony
/// becomes economically rational by paying ONLY for work the neutral panel rates
/// AT OR ABOVE the bar (no contract change: a sub-bar result is simply NOT
/// accepted, so its escrow stays locked and is `reclaimExpired`-recoverable after
/// the ttl). Rule: `median >= min`. With the default `min = 2`, a median of 1
/// (the clear-failure / hallucination band) is REJECTED while 2..=5 are paid.
/// Inputs are clamped to the 1..=5 rating range so a stray 0 can never sneak a
/// payment past a `min = 1` floor.
pub(crate) fn should_accept(median: u8, min: u8) -> bool {
    median.clamp(1, 5) >= min.clamp(1, 5)
}

/// Default payment-gate threshold for `colony run` (`--min-accept-rating`). A
/// median of 1 (clear failure / hallucination) is rejected; 2..=5 are paid.
pub(crate) const COLONY_DEFAULT_MIN_ACCEPT: u8 = 2;

/// Parsed `colony run` arguments. The task is the joined positional remainder
/// (so an unquoted multi-word task works, matching `bounty post`).
pub(crate) struct ParsedColonyRun {
    task: String,
    reward_wei: u128,
    worker: Option<String>,
    /// An explicit single-judge override (`--judge <agent>`) — a panel of exactly
    /// that one neutral agent. `None` → auto-select a panel of `judges` agents.
    judge: Option<String>,
    /// Target panel size for the auto-selected NEUTRAL JUDGE PANEL (`--judges N`,
    /// default [`COLONY_DEFAULT_PANEL`]). Ignored when `judge` is set.
    judges: usize,
    /// PAYMENT GATE (`--min-accept-rating N`, default [`COLONY_DEFAULT_MIN_ACCEPT`]):
    /// the caller accepts + pays IFF the panel median is `>= min_accept`. A median
    /// below it is REJECTED (the worker is NOT paid; the escrow is reclaimable after
    /// the ttl). Validated to 1..=5 at parse time.
    min_accept: u8,
    ttl_secs: u64,
}

/// Judge WALLET-balance floor (0.25 `$LH`) below which the colony tops a judge
/// up before its turn: a judge under this can't reliably fund the lazy meter
/// deposit for its metered turn and would 402 out of the panel. When tripped
/// the colony sends a fixed 0.5 `$LH` top-up (see the `send_lh` call below) —
/// comfortably above the floor so one top-up covers several turns.
pub(crate) const JUDGE_FUND_FLOOR_WEI: u128 = 250_000_000_000_000_000;

/// Default neutral-judge panel size for `colony run` (median of N). Odd so the
/// median is a clean middle value with no even-split tie.
pub(crate) const COLONY_DEFAULT_PANEL: usize = 3;

/// Parse `colony run` flags. Pure/testable — mirrors `parse_bounty_post_args`
/// plus a `--worker` override.
pub(crate) fn parse_colony_run_args(rest: &[String]) -> Result<ParsedColonyRun, String> {
    let ([reward, worker, judge, judges, min_accept, ttl], positional) = collect_flags(
        rest,
        ["--reward", "--worker", "--judge", "--judges", "--min-accept-rating", "--ttl"],
        COLONY_USAGE,
    )?;
    if positional.is_empty() {
        return Err(format!("colony run needs a <task>\n{COLONY_USAGE}"));
    }
    let task = positional.join(" ");
    let reward_label =
        reward.ok_or_else(|| format!("colony run needs --reward <X $LH>\n{COLONY_USAGE}"))?;
    let reward_wei = match localharness::encoding::parse_token_amount(&reward_label) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--reward must be a positive $LH amount, got '{reward_label}'")),
    };
    let ttl_secs = match ttl {
        None => INVITE_DEFAULT_TTL_SECS,
        Some(raw) => parse_ttl(&raw)?,
    };
    let judges = match judges {
        None => COLONY_DEFAULT_PANEL,
        Some(raw) => match raw.trim().parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => return Err(format!("--judges must be a positive integer, got '{raw}'")),
        },
    };
    // The PAYMENT GATE threshold (1..=5). Rejects 0 and out-of-band N so a median
    // can be compared against a real rating bar; default is the clear-failure floor.
    let min_accept = match min_accept {
        None => COLONY_DEFAULT_MIN_ACCEPT,
        Some(raw) => match raw.trim().parse::<u8>() {
            Ok(n) if (1..=5).contains(&n) => n,
            _ => return Err(format!("--min-accept-rating must be 1..5, got '{raw}'")),
        },
    };
    Ok(ParsedColonyRun { task, reward_wei, worker, judge, judges, min_accept, ttl_secs })
}

/// `localharness colony <subcommand>` — the colony-engine router.
pub(crate) async fn colony(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("run") => colony_run(caller, &rest[1..]).await,
        _ => {
            eprintln!("{COLONY_USAGE}");
            2
        }
    }
}

/// A drivable worker candidate for the colony PICK step: a discover-matched
/// agent whose key is local, decorated with on-chain reputation. `task_rank` is
/// its 0-based position in `discover_agents` (lower = better task fit; name-hit
/// before persona-hit, newest-first within a tier). `rep_count`/`rep_sum` are the
/// raw `reputationOf` pair (attestation count + rating sum, sum ≤ 5·count). Pure
/// data so the selection rule below is unit-testable with no network.
#[derive(Debug, Clone)]
pub(crate) struct WorkerCandidate {
    name: String,
    task_rank: usize,
    rep_count: u64,
    rep_sum: u64,
}

impl WorkerCandidate {
    /// Average rating in milli-units (so 5.0★ = 5000), `0` when never attested.
    /// Integer math keeps the selection rule exact + reproducible (no float
    /// ordering surprises). An unproven agent (count 0) sorts as avg 0 — below
    /// any proven one at the same task-fit tier, but still eligible.
    fn avg_milli(&self) -> u64 {
        // checked_div: an unproven agent (rep_count 0) averages 0.
        (self.rep_sum * 1000).checked_div(self.rep_count).unwrap_or(0)
    }
}

/// Candidates whose `task_rank` is within this many positions of the BEST
/// (rank-0) match are treated as "similar task fit" and decided on reputation.
/// Outside the band, the better task fit wins outright — so a wildly-irrelevant
/// high-reputation agent can never out-rank a clearly more task-relevant one.
/// Discover returns name-hits before persona-hits, so a small band keeps
/// reputation as the decider among genuinely comparable agents only.
pub(crate) const COLONY_TASK_FIT_BAND: usize = 3;

/// The reputation-aware selection RULE (pure + testable). Picks the best worker
/// from `candidates` (each already filtered to "task-relevant AND locally
/// keyed"). The blend, in strict priority order:
///   1. **Task-fit tier** (primary) — group candidates by discover proximity:
///      everything within `COLONY_TASK_FIT_BAND` positions of the top match is
///      one tier; a meaningfully worse task match is a lower tier. Better tier
///      always wins (task fit dominates).
///   2. **Average rating** (secondary) — within a tier, higher avg★ wins, so
///      proven good work beats unproven at comparable task fit.
///   3. **Attestation count** (tertiary tiebreak) — more attestations wins when
///      avg ties (a 5.0 from 3 beats a 5.0 from 1).
///   4. **Discover rank** (final tiebreak) — the original task-fit order, so the
///      result is deterministic.
///
/// An agent with NO attestations (avg 0) is eligible but ranks below a proven one
/// in the same tier. Returns `None` only for an empty slice.
pub(crate) fn pick_reputation_aware(candidates: &[WorkerCandidate]) -> Option<&WorkerCandidate> {
    let best_rank = candidates.iter().map(|c| c.task_rank).min()?;
    // Tier 0 = within the band of the best; higher tiers = progressively worse
    // task fit (one tier per band-width step beyond the best).
    let tier = |c: &WorkerCandidate| (c.task_rank - best_rank) / (COLONY_TASK_FIT_BAND + 1);
    candidates.iter().min_by(|a, b| {
        tier(a)
            .cmp(&tier(b)) // 1. lower tier (better task fit) first
            .then_with(|| b.avg_milli().cmp(&a.avg_milli())) // 2. higher avg★ first
            .then_with(|| b.rep_count.cmp(&a.rep_count)) // 3. more attestations first
            .then_with(|| a.task_rank.cmp(&b.task_rank)) // 4. better discover rank first
    })
}

/// A one-line, human-readable justification for a PICK — so the choice is
/// transparent in the colony transcript. Pure.
pub(crate) fn colony_pick_reasoning(c: &WorkerCandidate) -> String {
    let fit = if c.task_rank == 0 {
        "top task match".to_string()
    } else {
        format!("task match #{}", c.task_rank + 1)
    };
    if c.rep_count == 0 {
        format!("picked {} — no reputation yet, {} among local agents", c.name, fit)
    } else {
        let whole = c.avg_milli() / 1000;
        let frac = (c.avg_milli() % 1000) / 100; // one decimal place
        let plural = if c.rep_count == 1 { "attestation" } else { "attestations" };
        format!(
            "picked {} — reputation {whole}.{frac} from {} {plural} ({fit} among local agents)",
            c.name, c.rep_count
        )
    }
}

/// Pure: extract the significant search keywords from a free-form `task`, so a
/// descriptive task ("QA: suggest one concrete CLI improvement") still surfaces
/// relevant agents. `registry::discover_agents` matches the query as a SINGLE
/// substring, so feeding it the whole sentence matches nothing — we split into
/// words, lowercase, strip punctuation, drop short/stop words, and de-dupe
/// (preserving order). Capped at `COLONY_MAX_KEYWORDS` so the discovery fan-out
/// stays bounded. Empty when the task has no significant words.
pub(crate) fn colony_task_keywords(task: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "and", "or", "to", "of", "in", "on", "for", "with", "one", "two",
        "is", "are", "be", "this", "that", "your", "you", "it", "as", "at", "by", "from",
        "suggest", "please", "make", "give", "find", "do", "can", "should", "would", "about",
    ];
    let mut out: Vec<String> = Vec::new();
    for raw in task.split_whitespace() {
        let w: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        if w.len() < 3 || STOP.contains(&w.as_str()) || out.contains(&w) {
            continue;
        }
        out.push(w);
        if out.len() >= COLONY_MAX_KEYWORDS {
            break;
        }
    }
    out
}

/// Cap on keywords fanned out to `discover_agents` per task (bounds the reads).
pub(crate) const COLONY_MAX_KEYWORDS: usize = 6;

/// Discover task-relevant agents ROBUSTLY: try the full task as one query first
/// (cheap — catches an exact name/persona hit), then fan out across the task's
/// keywords ([`colony_task_keywords`]) and UNION the matches, preserving the
/// best rank each name was first seen at (so a name-hit / earlier-keyword agent
/// stays ahead). Returns `(name, persona)` rows in ascending rank order. This is
/// what lets `colony_pick_worker` find a worker for a descriptive task even
/// though `discover_agents` only does single-substring matching.
pub(crate) async fn colony_discover_relevant(task: &str) -> Result<Vec<(String, String)>, String> {
    // best rank seen per name + the persona; insertion order tracked separately.
    let mut best: std::collections::HashMap<String, (usize, String)> =
        std::collections::HashMap::new();
    let mut rank_cursor = 0usize;
    let mut absorb = |rows: Vec<(String, String)>, cursor: &mut usize| {
        for (name, persona) in rows {
            let r = *cursor;
            *cursor += 1;
            best.entry(name)
                .and_modify(|e| {
                    if r < e.0 {
                        e.0 = r;
                    }
                })
                .or_insert((r, persona));
        }
    };
    // 1. Full task verbatim (an exact persona/name hit ranks first).
    let full = registry::discover_agents(task, 100)
        .await
        .map_err(|e| format!("discover failed: {e}"))?;
    absorb(full, &mut rank_cursor);
    // 2. Per-keyword fan-out (keeps descriptive tasks discoverable).
    for kw in colony_task_keywords(task) {
        let rows = registry::discover_agents(&kw, 100)
            .await
            .map_err(|e| format!("discover failed: {e}"))?;
        absorb(rows, &mut rank_cursor);
    }
    let mut ranked: Vec<(String, (usize, String))> = best.into_iter().collect();
    ranked.sort_by_key(|(_, (rank, _))| *rank);
    Ok(ranked.into_iter().map(|(name, (_, persona))| (name, persona)).collect())
}

/// Pure: drop the `caller`'s own identity from a worker-candidate pool — the
/// colony picking the caller as the worker is a DEGENERATE self-deal (caller
/// posts → does the work → pays its OWN TBA, and the [8/8] self-attest reverts
/// on-chain). Matching is case-INSENSITIVE on the bare name (subdomain names are
/// case-insensitive). Returns the surviving candidates (order preserved), which
/// may be empty — the auto-PICK then fails with "no valid worker". Testable.
pub(crate) fn exclude_caller_candidates(candidates: Vec<WorkerCandidate>, caller: &str) -> Vec<WorkerCandidate> {
    candidates
        .into_iter()
        .filter(|c| !c.name.eq_ignore_ascii_case(caller))
        .collect()
}

/// Auto-pick the best worker for `task`, REPUTATION-AWARE. Builds the set of
/// drivable candidates (a `discover` match whose identity key is present locally,
/// so it can sign its own claim+submit), EXCLUDES the `caller` (no self-deal),
/// reads each one's on-chain reputation, then applies [`pick_reputation_aware`].
/// Returns `(name, reasoning_line)` so the caller can echo WHY this worker was
/// chosen, or an error naming what to do. Read-only (no `$LH`).
pub(crate) async fn colony_pick_worker(task: &str, caller: &str) -> Result<(String, String), String> {
    let matches = colony_discover_relevant(task).await?;
    if matches.is_empty() {
        return Err(
            "no agents matched the task to auto-pick a worker — pass --worker <agent> \
             (an agent whose key is in your keys dir)"
                .to_string(),
        );
    }
    // Drivable candidates only: a discover match we ALSO hold a key for (it must
    // sign its own claim + submit). `task_rank` = the merged discover position.
    // The CALLER is skipped up front (no wasted reputation RPC + no self-deal).
    let mut candidates: Vec<WorkerCandidate> = Vec::new();
    for (task_rank, (name, _persona)) in matches.iter().enumerate() {
        if resolve_key_read_path(name).is_none() {
            continue;
        }
        if name.eq_ignore_ascii_case(caller) {
            continue; // never auto-pick the caller as its own worker (self-deal).
        }
        // The candidate must be REGISTERED ON THE ACTIVE CHAIN. A local key whose
        // name resolves to tokenId 0 HERE is a cross-chain ghost — e.g. a testnet
        // agent when the cycle is running on mainnet — and picking it would strand
        // the cycle at CLAIM (it has no identity on this chain to claim with). A
        // confirmed `Ok(0)` excludes it; an RPC ERROR is transient and must NOT
        // drop an otherwise-drivable worker, so it stays (unproven 0,0 reputation).
        let (rep_count, rep_sum) = match registry::id_of_name(name).await {
            Ok(0) => continue, // not registered on this chain — skip the cross-chain ghost.
            Ok(id) => registry::reputation_of(id).await.unwrap_or((0, 0)),
            Err(_) => (0, 0), // transient RPC error — keep as unproven, don't drop.
        };
        candidates.push(WorkerCandidate {
            name: name.clone(),
            task_rank,
            rep_count,
            rep_sum,
        });
    }
    // Belt-and-braces: re-filter the pool (pure + tested) so the self-deal can
    // never slip through, then PICK. An empty pool after excluding the caller
    // means there's no valid worker — fail clearly rather than self-dealing.
    let candidates = exclude_caller_candidates(candidates, caller);
    match pick_reputation_aware(&candidates) {
        Some(c) => Ok((c.name.clone(), colony_pick_reasoning(c))),
        None => Err(format!(
            "no valid worker to auto-pick — the discover matches ({}) are either the caller \
             ({caller}) itself or have no local key. Pass --worker <agent> (NOT the caller) whose \
             key is in your keys dir (the worker signs its own claim + submit).",
            matches.iter().take(5).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Pure: choose up to `n` DISTINCT neutral judges from the locally-keyed agent
/// names `local`, EXCLUDING the `worker` and the `caller` (so neither the party
/// being rated nor the party that posted the work can score it — that's the
/// neutrality the panel buys). `local` is taken in its caller-supplied order
/// (`identity_key_files` sorts by name, so selection is deterministic); the first
/// `n` eligible names are taken. Returns fewer than `n` when too few neutral
/// agents exist (the caller notes the shortfall + still runs the smaller panel).
/// Empty only when there is NO neutral local agent at all. Testable with no fs.
pub(crate) fn select_judge_panel(local: &[String], worker: &str, caller: &str, n: usize) -> Vec<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut panel: Vec<String> = Vec::new();
    for name in local {
        if panel.len() >= n {
            break;
        }
        let s = name.as_str();
        if s == worker || s == caller || !seen.insert(s) {
            continue; // exclude the worker, the caller, and de-dupe.
        }
        panel.push(name.clone());
    }
    panel
}

/// Pure: `true` when an explicit `--judge <agent>` names the same identity as the
/// WORKER — which would let the worker grade its OWN work (self-inflated rating),
/// the exact self-deal the neutral panel exists to prevent. Case-INSENSITIVE on
/// the bare name (subdomain names are case-insensitive). Testable with no fs.
pub(crate) fn judge_equals_worker(judge: &str, worker: &str) -> bool {
    judge.eq_ignore_ascii_case(worker)
}

/// Resolve the NEUTRAL JUDGE PANEL for `colony run`: scan every locally-keyed
/// identity ([`identity_key_files`] → bare names), keep only those REGISTERED ON
/// THE ACTIVE CHAIN, and pick up to `n` DISTINCT neutral agents excluding the
/// `worker` AND the `caller`. Returns the panel names (each holds a local key, so
/// each funds + signs its own judge turn). On zero neutral agents this returns an
/// empty Vec; the caller falls back to the caller-as-judge so the cycle never
/// strands the escrow.
///
/// The registration filter is the fix for the cross-chain-ghost leak: a local key
/// whose name resolves to tokenId 0 here is an agent from ANOTHER chain (a testnet
/// agent on a mainnet run). Before this gate it entered the panel, got a wasted
/// 0.5 $LH top-up, then failed its turn as "not a registered agent". A confirmed
/// `Ok(0)` drops the name; an RPC error is transient so the name is KEPT (fail-open
/// on uncertainty, fail-closed only on a definite "not on this chain").
pub(crate) async fn resolve_judge_panel(worker: &str, caller: &str, n: usize) -> Vec<String> {
    let local: Vec<String> = identity_key_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            std::path::Path::new(&p)
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|f| f.strip_suffix(KEY_SUFFIX))
                .map(str::to_string)
        })
        .collect();
    // Drop cross-chain ghosts (tokenId 0 on the active chain) BEFORE selecting, so
    // the panel is built only from agents that can actually run a metered turn here.
    let mut registered: Vec<String> = Vec::new();
    for name in local {
        match registry::id_of_name(&name).await {
            Ok(0) => {}                  // not registered on this chain — drop it.
            _ => registered.push(name),  // registered, or a transient error → keep.
        }
    }
    select_judge_panel(&registered, worker, caller, n)
}

/// `true` if a sponsored-write error looks TRANSIENT (an RPC/transport hiccup,
/// not a contract revert) — worth one retry. The Tempo RPC intermittently fails
/// to decode the `eth_sendRawTransaction` RESPONSE even when the tx mined, so we
/// re-check on-chain state before retrying (the caller does that). Pure.
pub(crate) fn is_transient_rpc_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    (e.contains("decode") || e.contains("decoding") || e.contains("timed out")
        || e.contains("timeout") || e.contains("connection") || e.contains("response body")
        || e.contains("eof"))
        // A real on-chain revert is NOT transient — those carry a reason/selector.
        && !e.contains("revert") && !e.contains("execution reverted")
}

/// `true` if a `run_agent_turn` outcome (the WORK / model turn) should be RETRIED
/// once. A retry is warranted when the model returned an EMPTY/whitespace-only
/// reply (a transient proxy/model hiccup that strands the claimed bounty — seen
/// dogfooding) OR the turn FAILED with a transient RPC/transport error (reusing
/// [`is_transient_rpc_error`] detection). A NON-empty reply is NEVER retried (it's
/// a real result), and a non-transient error (a genuine failure) bails as before.
/// Pure so the retry decision is unit-testable with no network.
pub(crate) fn work_result_needs_retry(outcome: &Result<(String, Option<Vec<u8>>), String>) -> bool {
    match outcome {
        Ok((text, _)) => text.trim().is_empty(),
        Err(e) => is_transient_rpc_error(e),
    }
}

/// Drive a `colony` on-chain WRITE step with ONE transient-error retry that's
/// guarded by an idempotence check: before retrying, read `getBounty(id).status`
/// and treat it as success if the chain ALREADY advanced past `done_at_or_after`
/// (the original tx mined; the failure was only the response decode). This is the
/// fix for the live decode-error-at-accept seen dogfooding the cycle — without it
/// a flaky RPC stranded the escrow in `Submitted`. `attempt` runs the sponsored
/// write; `step`/`verb` label the output. Returns the tx hash (or "(already
/// advanced on-chain)") on success, or a final error string on real failure.
pub(crate) async fn colony_write_step<F, Fut>(
    bounty_id: u64,
    step: &str,
    verb: &str,
    done_at_or_after: u8,
    attempt: F,
) -> Result<String, String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    match attempt().await {
        Ok(tx) => Ok(tx),
        Err(e) if is_transient_rpc_error(&e) => {
            eprintln!("      … {step} {verb}: transient RPC error ({e}); re-checking on-chain state …");
            // Did the original tx actually mine despite the bad response?
            if let Ok(b) = registry::get_bounty(bounty_id).await {
                if b.status >= done_at_or_after && b.status != 4 && b.status != 5 {
                    return Ok("(already advanced on-chain — the original tx mined)".to_string());
                }
            }
            eprintln!("      … retrying {step} {verb} once …");
            attempt().await
        }
        Err(e) => Err(e),
    }
}

/// Surface a mid-cycle `colony run` failure with the CORRECT escrow-recovery
/// command for the bounty's LIVE on-chain status. `cancelBounty` only works while
/// the bounty is OPEN — once a worker has CLAIMED it (status ≥ 1) cancel reverts
/// `NotOpen`, and the only recovery is the ttl-gated `reclaimExpired`
/// (`bounty reclaim`). Re-reading `getBounty(id).status` makes the advice right
/// even when a claim's tx mined but its RESPONSE decode failed (status = Claimed).
/// Returns the process exit code (always `1` — a failed cycle).
pub(crate) async fn colony_bail(bounty_id: u64, caller_label: &str, stage: &str, err: &str) -> i32 {
    eprintln!("[{stage}] {err}");
    let status = registry::get_bounty(bounty_id).await.ok().map(|b| b.status);
    eprintln!("{}", colony_recovery_hint(bounty_id, caller_label, status));
    eprintln!("  Inspect: localharness bounty mine --as {caller_label}");
    1
}

/// Pure: pick the CORRECT escrow-recovery hint for a stranded bounty given its
/// live on-chain `status` (`None` = the status read itself failed). The crux:
/// `bounty cancel` (`cancelBounty`) is accepted ONLY while OPEN (status 0) — once
/// CLAIMED/SUBMITTED (1/2) it reverts `NotOpen`, so the only recovery is the
/// ttl-gated `bounty reclaim` (`reclaimExpired`). Paid (3) / Cancelled (4) /
/// Reclaimed (5) are terminal (nothing to recover). On an unknown/unreadable
/// status, advise BOTH so the user is never stuck. Testable with no network.
pub(crate) fn colony_recovery_hint(bounty_id: u64, caller_label: &str, status: Option<u8>) -> String {
    match status {
        Some(0) => format!(
            "  ⚠ bounty #{bounty_id} is OPEN and unsettled. Recover the $LH now with:\n    \
             localharness bounty cancel --as {caller_label} {bounty_id}"
        ),
        Some(s @ (1 | 2)) => {
            let st = if s == 1 { "claimed" } else { "submitted" };
            format!(
                "  ⚠ bounty #{bounty_id} is {st} (already past OPEN) so `bounty cancel` would \
                 revert — the escrow refunds only after the ttl. Recover the $LH with:\n    \
                 localharness bounty reclaim --as {caller_label} {bounty_id}   (works once the ttl has expired)"
            )
        }
        Some(3) => format!(
            "  bounty #{bounty_id} is already PAID — the reward settled to the worker; nothing to recover."
        ),
        Some(4) | Some(5) => format!(
            "  bounty #{bounty_id} is already refunded (cancelled/reclaimed); nothing to recover."
        ),
        _ => format!(
            "  ⚠ bounty #{bounty_id} is escrowed and unsettled. If it is still OPEN: \
             `localharness bounty cancel --as {caller_label} {bounty_id}`; if a worker has already \
             claimed it, wait for the ttl then `localharness bounty reclaim --as {caller_label} {bounty_id}`."
        ),
    }
}

/// The [2/8] PICK step's output: the resolved, drivable worker — its name, its
/// OWN signing key (it signs its own claim + submit), the identity (tokenId)
/// that earns the reward, the TBA the reward lands in, and that TBA's `$LH`
/// balance BEFORE the cycle (for the final payout verification).
struct ColonyWorker {
    name: String,
    signer: k256::ecdsa::SigningKey,
    token_id: u64,
    tba: String,
    tba_before: u128,
}

/// [1/8] POST — the caller escrows the reward behind the task and the new
/// bounty id is read back from its `bountiesOf` index. Returns the bounty id,
/// or the process exit code when the post (or the id read-back) failed —
/// before an id exists there is no escrow to recover, so this step prints its
/// own errors instead of bailing through `colony_bail`.
async fn colony_step_post(
    caller_signer: &k256::ecdsa::SigningKey,
    sponsor: &k256::ecdsa::SigningKey,
    caller_addr: &str,
    task: &str,
    reward_wei: u128,
    ttl_secs: u64,
) -> Result<u64, i32> {
    println!("[1/8] POST  — escrowing {} behind the task …", fmt_lh(reward_wei));
    // The escrow pulls the reward from the WALLET pot — auto-bridge any
    // shortfall out of the chat meter first (on-chain feedback #63).
    ensure_wallet_covers(caller_signer, caller_addr, reward_wei).await?;
    let post_tx = match registry::post_bounty_sponsored(
        caller_signer,
        sponsor,
        task.as_bytes(),
        reward_wei,
        ttl_secs,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("[1/8] POST failed: {e}");
            eprintln!("  no escrow was created — nothing to clean up.");
            return Err(1);
        }
    };
    // The new bounty id is the last entry in the poster's bountiesOf index.
    let bounty_id = match registry::bounties_of(caller_addr).await {
        Ok(ids) if !ids.is_empty() => ids[ids.len() - 1],
        Ok(_) => {
            eprintln!(
                "[1/8] POST mined (tx {post_tx}) but the new bounty id could not be read back \
                 from bountiesOf — re-run `bounty mine` to find + manage it."
            );
            return Err(1);
        }
        Err(e) => {
            eprintln!(
                "[1/8] POST mined (tx {post_tx}) but reading the bounty id failed: {e} \
                 — re-run `bounty mine` to find + manage it."
            );
            return Err(1);
        }
    };
    println!("      ✓ bounty #{bounty_id} posted  (tx {post_tx})");
    println!();
    Ok(bounty_id)
}

/// [2/8] PICK — resolve the worker (an explicit `--worker`, else the
/// reputation-aware auto-pick), guard the `--judge`-names-the-worker self-deal,
/// and load the worker's OWN key + tokenId + TBA (with its before-balance for
/// the final payout check). `Err` carries the stage-2 bail message.
async fn colony_step_pick(
    worker: Option<String>,
    judge: &Option<String>,
    task: &str,
    caller_label: &str,
) -> Result<ColonyWorker, String> {
    let worker_name = match worker {
        Some(w) => w,
        None => {
            println!("[2/8] PICK  — auto-selecting the best worker (reputation-aware, excluding the caller) …");
            match colony_pick_worker(task, caller_label).await {
                Ok((w, why)) => {
                    println!("      ✓ {why}");
                    w
                }
                Err(e) => return Err(format!("PICK failed: {e}")),
            }
        }
    };
    // FIX 3: an explicit `--judge <agent>` must NOT name the WORKER. The auto-panel
    // already excludes the worker (+ caller), but the override bypassed that — a
    // caller could force the worker to judge its OWN work (self-inflated rating).
    // Reject up front (clearest), keeping `--judge <neutral-agent>` working.
    if let Some(j) = judge {
        if judge_equals_worker(j, &worker_name) {
            return Err(format!(
                "--judge '{j}' is the WORKER — a worker can't judge its own work (self-inflated \
                 rating). Pass --judge <neutral-agent> (NOT the worker), or drop --judge to use \
                 the auto-selected neutral panel."
            ));
        }
    }
    // The worker signs its OWN claim + submit, so its key must be local.
    let (worker_key_file, worker_key_hex) = match resolve_caller_key(Some(&worker_name)) {
        Ok(c) => c,
        Err(e) => {
            return Err(format!(
                "worker '{worker_name}' has no local identity key ({e}). The worker must be a \
                 fleet/owned agent whose key is in your keys dir — it signs its own claim + submit."
            ))
        }
    };
    let signer = match wallet::from_private_key_hex(&worker_key_hex) {
        Ok(s) => s,
        Err(e) => return Err(format!("bad worker key in {worker_key_file}: {e}")),
    };
    // The worker's tokenId (the identity that earns the reward) + its TBA wallet
    // (where the reward lands) — resolve both up front so the payout is verifiable.
    let worker_token_id = match resolve_own_token_id(Some(&worker_name), &signer).await {
        Ok(id) => id,
        Err(e) => return Err(format!("could not resolve worker '{worker_name}' identity: {e}")),
    };
    let worker_tba = match registry::tba_of_token_id(worker_token_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return Err(format!("worker token #{worker_token_id} has no token-bound account")),
        Err(e) => return Err(format!("RPC error resolving worker TBA: {e}")),
    };
    let tba_before = registry::token_balance_of(&worker_tba).await.unwrap_or(0);
    println!("      worker {worker_name} = token #{worker_token_id}, TBA {worker_tba}");
    println!("      worker TBA $LH before: {}", fmt_lh(tba_before));
    println!();
    Ok(ColonyWorker {
        name: worker_name,
        signer,
        token_id: worker_token_id,
        tba: worker_tba,
        tba_before,
    })
}

/// [3/8] CLAIM — the worker claims the bounty under its own key (one
/// transient-retry via [`colony_write_step`]). `Err` = the stage-3 bail message.
async fn colony_step_claim(
    worker: &ColonyWorker,
    sponsor: &k256::ecdsa::SigningKey,
    bounty_id: u64,
) -> Result<(), String> {
    println!(
        "[3/8] CLAIM — {worker_name} claims bounty #{bounty_id} (reward → its TBA) …",
        worker_name = worker.name
    );
    match colony_write_step(bounty_id, "3/8", "CLAIM", 1, || {
        registry::claim_bounty_sponsored(
            &worker.signer,
            sponsor,
            bounty_id,
            worker.token_id,
            registry::ALPHA_USD_ADDRESS(),
        )
    })
    .await
    {
        Ok(tx) => {
            println!("      ✓ claimed by token #{}  (tx {tx})", worker.token_id);
            println!();
            Ok(())
        }
        Err(e) => Err(format!("CLAIM failed: {e}")),
    }
}

/// [4/8] WORK — run the worker's on-chain persona on the task (a headless
/// `call` turn, paid by the caller key). Retries ONCE on an empty reply or a
/// transient failure; returns the trimmed result text, or the stage-4 bail
/// message.
async fn colony_step_work(
    caller_key_hex: &str,
    worker_name: &str,
    task: &str,
) -> Result<String, String> {
    println!("[4/8] WORK  — running {worker_name}'s persona on the task (headless `call`) …");
    let work_prompt = format!(
        "{task}\n\nSubmit your concrete result / deliverable as your reply \
         (it will be recorded on-chain as your bounty submission)."
    );
    // The caller pays for the work turn (same as `call --as caller worker …`),
    // running the WORKER's on-chain persona. No prior history (a one-shot job).
    // Retry ONCE on an EMPTY reply or a TRANSIENT failure: tick-14's dogfood saw
    // the WORK turn return an empty model reply (a transient proxy/model hiccup)
    // that bailed the whole cycle and stranded the claimed bounty. A non-empty
    // result is NEVER retried; a genuine (non-transient) error bails as before.
    let mut work_outcome =
        run_agent_turn(caller_key_hex, worker_name, &work_prompt, None, None).await;
    if work_result_needs_retry(&work_outcome) {
        match &work_outcome {
            Ok(_) => println!(
                "      ⚠ WORK returned an empty reply (transient model hiccup) — retrying once …"
            ),
            Err(e) => println!(
                "      ⚠ WORK turn failed transiently ({e}) — retrying once …"
            ),
        }
        work_outcome =
            run_agent_turn(caller_key_hex, worker_name, &work_prompt, None, None).await;
    }
    let result_text = match work_outcome {
        Ok((text, _hist)) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                return Err("WORK produced an empty result (even after one retry) — nothing to submit.".to_string());
            }
            trimmed
        }
        Err(e) => {
            report_call_error("[4/8] WORK failed", &e);
            return Err("the worker's persona turn failed (even after one retry) — see the hint above.".to_string());
        }
    };
    println!("      ✓ {worker_name} produced a result:");
    println!("      ┌─────────────────────────────────────────────────────────");
    for line in result_text.lines() {
        println!("      │ {line}");
    }
    println!("      └─────────────────────────────────────────────────────────");
    println!();
    Ok(result_text)
}

/// [5/8] SUBMIT — the worker submits its result (one transient-retry via
/// [`colony_write_step`]). `Err` = the stage-5 bail message.
async fn colony_step_submit(
    worker: &ColonyWorker,
    sponsor: &k256::ecdsa::SigningKey,
    bounty_id: u64,
    result_text: &str,
) -> Result<(), String> {
    println!(
        "[5/8] SUBMIT — {worker_name} submits its result for bounty #{bounty_id} …",
        worker_name = worker.name
    );
    match colony_write_step(bounty_id, "5/8", "SUBMIT", 2, || {
        registry::submit_result_sponsored(
            &worker.signer,
            sponsor,
            bounty_id,
            result_text.as_bytes(),
            registry::ALPHA_USD_ADDRESS(),
        )
    })
    .await
    {
        Ok(tx) => {
            println!("      ✓ result submitted  (tx {tx})");
            println!();
            Ok(())
        }
        Err(e) => Err(format!("SUBMIT failed: {e}")),
    }
}

/// [6/8] JUDGE — a NEUTRAL JUDGE PANEL scores the result; returns the MEDIAN.
/// This is what makes the attestation MEANINGFUL and TRUSTWORTHY: the rating
/// is the MEDIAN of N neutral judges (default 3), not one self-interested
/// score. The panel EXCLUDES the worker (don't grade your own work) AND the
/// caller (the poster has skin in the game — its score would bias the
/// reputation signal that now DRIVES the PICK step). `--judge <agent>` forces a
/// panel of exactly that one named agent. Each judge signs + funds its OWN turn
/// (its key is local); the judge agent's PERSONA is embodied but the impartial
/// PROMPT overrides its framing. A failed judge turn doesn't bail (the payout
/// still happens) — and if ALL judges fail the median falls back to a neutral 3
/// so the cycle completes with an honest, non-inflated rating.
async fn colony_step_judge(
    judge: &Option<String>,
    judges: usize,
    worker_name: &str,
    caller_label: &str,
    caller_key_hex: &str,
    task: &str,
    result_text: &str,
) -> u8 {
    // Build the panel: an explicit `--judge X` = the single agent X; else
    // auto-select up to `judges` neutral local agents (excluding worker + caller).
    let panel: Vec<String> = match judge {
        Some(j) => vec![j.clone()],
        None => resolve_judge_panel(worker_name, caller_label, judges).await,
    };
    println!(
        "[6/8] JUDGE — neutral panel scores {worker_name}'s result 1-5 (accuracy-checked) …"
    );
    if judge.is_none() {
        if panel.is_empty() {
            // No neutral local agent — fall back to the caller as a single judge
            // (better an interested score than stranding the cycle). Loud note.
            println!(
                "      ⚠ no neutral local agent (excluding the worker + caller) to form a panel; \
                 falling back to the caller ({caller_label}) as a single judge."
            );
        } else if panel.len() < judges {
            println!(
                "      note: only {} neutral local agent(s) available (asked for {judges}); \
                 running a panel of {}.",
                panel.len(),
                panel.len()
            );
        }
    }
    // Run each judge in turn, collecting (label, rating, rationale). A judge whose
    // turn FAILS is skipped (logged) — it doesn't pollute the median with a
    // fabricated score. The caller key pays the fallback (caller-as-judge) turn.
    let judge_prompt = colony_judge_prompt(task, result_text);
    let mut panel_results: Vec<(String, u8, String)> = Vec::new();
    // The effective panel: the resolved neutral agents, or — when empty — the
    // caller acting as the lone judge (paid by the caller key already loaded).
    let effective_panel: Vec<String> =
        if panel.is_empty() { vec![caller_label.to_string()] } else { panel.clone() };
    for judge_name in &effective_panel {
        // Each neutral judge funds + signs its own turn; the caller-fallback judge
        // reuses the caller key (so a missing-key judge can't strand the escrow).
        let judge_key_hex = if judge_name.as_str() == caller_label {
            caller_key_hex.to_string()
        } else {
            let hex = match resolve_caller_key(Some(judge_name)) {
                Ok((_, hex)) => hex,
                Err(e) => {
                    eprintln!(
                        "      ⚠ judge '{judge_name}' has no local identity key ({e}); skipping it."
                    );
                    continue;
                }
            };
            // A judge with an empty wallet 402s its metered turn and SILENTLY
            // shrinks the panel (seen live: 2 of 3 judges excluded → a 1-judge
            // "panel"). Best-effort top-up from the CALLER — who already pays
            // for the cycle — when the judge's wallet is under the metering
            // floor (the lazy meter deposit pulls 0.2 $LH from the judge's own
            // wallet). A failed send degrades to today's behavior (excluded).
            if let Ok(signer) = wallet::from_private_key_hex(&hex) {
                let judge_addr = bytes_to_hex_str(&wallet::address(&signer));
                if matches!(
                    registry::token_balance_of(&judge_addr).await,
                    Ok(b) if b < JUDGE_FUND_FLOOR_WEI
                ) {
                    println!(
                        "      · judge '{judge_name}' wallet is under the metering floor — \
                         funding 0.5 $LH from {caller_label}"
                    );
                    // Fund the ADDRESS just balance-checked — the local key's,
                    // which signs (and is metered for) the judge turn. Sending
                    // to the NAME resolves the on-chain owner instead, which
                    // diverges from the local key exactly when keys go stale
                    // (the rho-qa class): a no-op for an unregistered name, or
                    // 0.5 $LH misdirected to a stranger who re-registered it.
                    // React to a failed top-up (the exit code was discarded before —
                    // send_lh prints the reason, but the colony ignored it and ran the
                    // judge anyway). A non-zero code means the judge will likely 402
                    // out; the shortfall summary below then explains the shrunk panel.
                    if credits::send_lh(Some(caller_label), &judge_addr, "0.5").await != 0 {
                        eprintln!("      ⚠ judge '{judge_name}' top-up did not succeed; it may 402 out of the panel.");
                    }
                }
            }
            hex
        };
        match run_agent_turn(&judge_key_hex, judge_name, &judge_prompt, None, None).await {
            Ok((reply, _hist)) => {
                let (rating, rationale) = parse_judge_rating(&reply);
                let rating = rating.clamp(1, 5);
                println!("      • {judge_name}: {rating}★");
                if !rationale.is_empty() {
                    println!("        {rationale}");
                }
                panel_results.push((judge_name.clone(), rating, rationale));
            }
            Err(e) => {
                report_call_error(&format!("[6/8] JUDGE turn failed ({judge_name})"), &e);
                println!("      ⚠ judge '{judge_name}' turn failed — excluded from the median.");
            }
        }
    }
    // If the panel that actually SCORED is smaller than the one we set out to run
    // (a judge lacked a local key, failed its top-up, or 402'd/errored its turn),
    // say so — a silently-shrunk panel weakens the median and was invisible before.
    let planned = effective_panel.len();
    let actual = panel_results.len();
    if actual < planned {
        println!(
            "      note: {actual} of {planned} judges scored (the rest lacked a key, failed a \
             top-up, or errored) — the median rests on {actual}."
        );
    }
    // Aggregate to the MEDIAN. If EVERY judge turn failed, `median_rating([])`
    // returns the neutral 3 default — the cycle still completes with an honest,
    // non-inflated rating (the worker is never credited a false 5★).
    let panel_ratings: Vec<u8> = panel_results.iter().map(|(_, r, _)| *r).collect();
    let judged_rating = median_rating(&panel_ratings).clamp(1, 5);
    if panel_ratings.is_empty() {
        println!(
            "      ⚠ every judge turn failed — defaulting to a neutral {judged_rating}★ \
             (the cycle still completes; the worker is not credited a false 5★)."
        );
    } else {
        // Echo the panel + the median, e.g. "panel: dex-qa 5★, iris-qa 4★ → median 5★".
        let summary = panel_results
            .iter()
            .map(|(n, r, _)| format!("{n} {r}★"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("      ✓ panel: {summary} → median {judged_rating}★");
    }
    println!();
    judged_rating
}

/// [7/8] the PAYMENT GATE — ACCEPT (pay) the work or REJECT it, per the
/// already-computed `accept` decision ([`should_accept`]). The colony is
/// economically rational: it pays ONLY for work the NEUTRAL panel rates at or
/// above the `--min-accept-rating` bar (default 2). A median BELOW the bar is
/// REJECTED — the caller does NOT accept, so the worker is NOT paid and the
/// escrow stays locked, recoverable by the poster via `reclaimExpired`
/// (`bounty reclaim`) after the ttl. NO contract change: a reject is simply the
/// absence of an accept on a Submitted bounty. `Err` = the stage-7 bail message
/// (only the accept write can fail; a reject never touches the chain).
#[allow(clippy::too_many_arguments)]
async fn colony_step_settle(
    accept: bool,
    caller_signer: &k256::ecdsa::SigningKey,
    sponsor: &k256::ecdsa::SigningKey,
    bounty_id: u64,
    worker_name: &str,
    caller_label: &str,
    reward_wei: u128,
    judged_rating: u8,
    min_accept: u8,
) -> Result<(), String> {
    if accept {
        println!(
            "[7/8] ACCEPT — median {judged_rating}★ ≥ min {min_accept}★ → caller accepts + pays the \
             escrow to {worker_name}'s TBA …"
        );
        match colony_write_step(bounty_id, "7/8", "ACCEPT", 3, || {
            registry::accept_result_sponsored(
                caller_signer,
                sponsor,
                bounty_id,
                registry::ALPHA_USD_ADDRESS(),
            )
        })
        .await
        {
            Ok(tx) => println!("      ✓ accepted — {} settled  (tx {tx})", fmt_lh(reward_wei)),
            Err(e) => return Err(format!("ACCEPT failed: {e}")),
        }
    } else {
        // REJECT: the work scored below the bar. Do NOT accept/pay — the worker
        // keeps NOTHING. The escrow remains locked on the Submitted bounty; the
        // poster recovers it via the ttl-gated `bounty reclaim`. This is a NORMAL
        // outcome (a rational colony refusing sub-quality work), not an error.
        println!(
            "[7/8] REJECT — median {judged_rating}★ < min {min_accept}★ → caller does NOT accept; \
             {worker_name} is NOT paid."
        );
        println!("      ✗ result REJECTED ({judged_rating}★ below the {min_accept}★ bar).");
        println!("      ✗ the escrow ({}) was NOT released — the worker keeps NOTHING.", fmt_lh(reward_wei));
        println!(
            "      the escrow is reclaimable by the poster AFTER the ttl with:\n        \
             localharness bounty reclaim --as {caller_label} {bounty_id}"
        );
    }
    println!();
    Ok(())
}

/// [8/8] ATTEST — the caller attests the JUDGE's median rating to the worker
/// (workRef = the bounty id). ALWAYS runs, accept OR reject: reputation must
/// reflect the work's true quality (a rejected 1★ result is recorded as 1★, so
/// the bad worker's reputation drops and the PICK step routes around it next
/// time). Attestation is reputation, not payment, so it is the SAME on both
/// branches. A failure here WARNS but does NOT fail the cycle (and never
/// triggers a bail — on the accept branch the escrow is settled; on the reject
/// branch it is reclaimable).
async fn colony_step_attest(
    caller_signer: &k256::ecdsa::SigningKey,
    sponsor: &k256::ecdsa::SigningKey,
    worker: &ColonyWorker,
    bounty_id: u64,
    judged_rating: u8,
    caller_label: &str,
) {
    let (worker_name, worker_token_id) = (&worker.name, worker.token_id);
    println!(
        "[8/8] ATTEST — caller attests {judged_rating}★ (the JUDGE's rating) to {worker_name} \
         (workRef = bounty #{bounty_id}) …"
    );
    let work_ref = bounty_work_ref(bounty_id);
    match registry::attest_sponsored(
        caller_signer,
        sponsor,
        worker_token_id,
        judged_rating,
        work_ref,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => println!(
            "      ✓ attested {judged_rating}★ to {worker_name} (token #{worker_token_id})  (tx {tx})"
        ),
        Err(e) => println!(
            "      ⚠ ATTEST failed: {e}\n      \
             (attestation is a bonus; not failing the cycle. \
             Retry later with: localharness reputation attest --as {caller_label} {worker_name} {judged_rating} --ref {bounty_id})"
        ),
    }
    println!();
}

/// The closing report — verify the outcome against the worker's TBA `$LH` and
/// print the ACCEPTED / REJECTED cycle summary (the reject branch's delta check
/// is the KEY PROOF of the payment gate: a rejected result never moves `$LH`).
async fn colony_report_outcome(
    accept: bool,
    bounty_id: u64,
    worker: &ColonyWorker,
    reward_wei: u128,
    judged_rating: u8,
    min_accept: u8,
    caller_label: &str,
) {
    let (worker_name, worker_tba, tba_before) = (&worker.name, &worker.tba, worker.tba_before);
    let tba_after = registry::token_balance_of(worker_tba).await.unwrap_or(tba_before);
    let delta = tba_after.saturating_sub(tba_before);
    if accept {
        println!("=== CYCLE COMPLETE (ACCEPTED) ===");
        println!("  bounty #{bounty_id}: open → claimed → submitted → accepted → PAID");
        println!("  worker TBA {worker_tba}");
        println!("    before: {}", fmt_lh(tba_before));
        println!("    after:  {}", fmt_lh(tba_after));
        println!("    delta:  +{}  (reward {})", fmt_lh(delta), fmt_lh(reward_wei));
        if delta == reward_wei {
            println!("  ✓ payout verified — the worker's TBA rose by exactly the reward.");
        } else {
            // The cycle COMPLETED on-chain (accept mined); a balance read can lag a
            // block or another tx can touch the TBA. Report honestly, don't fail the
            // accepted cycle — the escrow is settled either way.
            println!(
                "  ⚠ TBA delta ({}) != reward ({}). The accept tx mined (the bounty is PAID), \
                 but the balance check didn't line up exactly — a read can lag a block or another \
                 tx touched the TBA. Re-check with: localharness tba show {worker_name}",
                fmt_lh(delta),
                fmt_lh(reward_wei)
            );
        }
    } else {
        // The KEY PROOF of the gate: a rejected result NEVER moves $LH to the
        // worker's TBA. The cycle ended on a Submitted (not Paid) bounty.
        println!("=== CYCLE COMPLETE (REJECTED — NOT PAID) ===");
        println!("  bounty #{bounty_id}: open → claimed → submitted → REJECTED (still Submitted, escrow locked)");
        println!("  worker TBA {worker_tba}");
        println!("    before: {}", fmt_lh(tba_before));
        println!("    after:  {}", fmt_lh(tba_after));
        println!("    delta:  +{}  (NO payout — median {judged_rating}★ < min {min_accept}★)", fmt_lh(delta));
        if delta == 0 {
            println!("  ✓ reject verified — the worker's TBA did NOT rise (it was not paid).");
        } else {
            println!(
                "  ⚠ the worker's TBA rose by {} despite the reject — the colony did NOT accept \
                 this bounty, so this delta came from ANOTHER tx, not this reward. Re-check with: \
                 localharness tba show {worker_name}",
                fmt_lh(delta)
            );
        }
        println!(
            "  the escrow stays locked on the Submitted bounty; reclaim it after the ttl with:\n    \
             localharness bounty reclaim --as {caller_label} {bounty_id}"
        );
    }
}

/// `colony run` — drive ONE autonomous post→claim→work→submit→JUDGE→
/// (accept-or-reject)→attest cycle. Each on-chain step reuses the bounty helpers;
/// the work AND the judge both reuse `run_agent_turn`. The [6/8] JUDGE step scores
/// the worker's result 1-5 for genuine + accurate task-fit; [7/8] is the PAYMENT
/// GATE — the caller accepts + pays ONLY when the panel median is `>=
/// --min-accept-rating` (default 2), else REJECTS (no payment; the escrow stays
/// locked, reclaimable via `bounty reclaim` after the ttl). [8/8] ATTEST signs the
/// panel median on-chain (not a hardcoded 5★) on BOTH branches — so reputation
/// reflects judged quality even for rejected work. A reject is a NORMAL outcome
/// (exit 0). On any failure mid-cycle the bounty id is surfaced so the escrow is
/// never silently stranded.
pub(crate) async fn colony_run(caller: Option<&str>, rest: &[String]) -> i32 {
    let ParsedColonyRun { task, reward_wei, worker, judge, judges, min_accept, ttl_secs } =
        match parse_colony_run_args(rest) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
    if task.trim().is_empty() {
        eprintln!("colony run: task is empty");
        return 2;
    }

    // The caller (platform / poster) — its key signs the post + accept and pays
    // the headless `call` that runs the work.
    let (caller_signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let caller_addr = bytes_to_hex_str(&wallet::address(&caller_signer));
    let caller_label = match resolve_caller_label(caller) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("colony run: {e}");
            return 2;
        }
    };
    // The caller key (hex) drives the headless work turn (proxy auth + $LH).
    let caller_key_hex = match resolve_caller_key(caller) {
        Ok((_, hex)) => hex,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    println!("=== COLONY RUN — one autonomous agent-economy cycle ===");
    println!("  caller (poster): {caller_label}  ({caller_addr})");
    println!("  task:            {task}");
    println!("  reward:          {}", fmt_lh(reward_wei));
    println!();

    // -- STEP 1: the caller POSTS the bounty (escrows the reward). ----------
    let bounty_id = match colony_step_post(
        &caller_signer,
        &sponsor,
        &caller_addr,
        &task,
        reward_wei,
        ttl_secs,
    )
    .await
    {
        Ok(id) => id,
        Err(code) => return code,
    };

    // From here on, any failure must surface the bounty id so the escrow can be
    // reclaimed — never a silent half-state. The recovery COMMAND depends on the
    // bounty's live on-chain status: `cancelBounty` only works while OPEN (it
    // reverts `NotOpen` once a worker has CLAIMED), so a failure AFTER the claim
    // mined must steer the user to the EXPIRY → `bounty reclaim` path instead.
    // `colony_bail` re-reads `getBounty(id).status` so the advice is correct even
    // when a claim's tx mined but its response decode failed (status = Claimed).
    macro_rules! bail {
        ($stage:expr, $err:expr) => {
            return colony_bail(bounty_id, &caller_label, $stage, &$err).await
        };
    }

    // -- STEP 2: pick + resolve the WORKER. --------------------------------
    let worker = match colony_step_pick(worker, &judge, &task, &caller_label).await {
        Ok(w) => w,
        Err(e) => bail!("2/8", e),
    };

    // -- STEP 3: the worker CLAIMS the bounty. -----------------------------
    if let Err(e) = colony_step_claim(&worker, &sponsor, bounty_id).await {
        bail!("3/8", e);
    }

    // -- STEP 4: run the WORK — a headless turn as the worker's persona. ----
    let result_text = match colony_step_work(&caller_key_hex, &worker.name, &task).await {
        Ok(r) => r,
        Err(e) => bail!("4/8", e),
    };

    // -- STEP 5: the worker SUBMITS the result. ----------------------------
    if let Err(e) = colony_step_submit(&worker, &sponsor, bounty_id, &result_text).await {
        bail!("5/8", e);
    }

    // -- STEP 6: a NEUTRAL JUDGE PANEL scores the result; take the MEDIAN. ---
    let judged_rating = colony_step_judge(
        &judge,
        judges,
        &worker.name,
        &caller_label,
        &caller_key_hex,
        &task,
        &result_text,
    )
    .await;

    // -- STEP 7: the PAYMENT GATE — ACCEPT (pay) the work OR REJECT it. -----
    let accept = should_accept(judged_rating, min_accept);
    if let Err(e) = colony_step_settle(
        accept,
        &caller_signer,
        &sponsor,
        bounty_id,
        &worker.name,
        &caller_label,
        reward_wei,
        judged_rating,
        min_accept,
    )
    .await
    {
        bail!("7/8", e);
    }

    // -- STEP 8: the caller ATTESTS the JUDGE'S rating → on-chain reputation. -
    colony_step_attest(&caller_signer, &sponsor, &worker, bounty_id, judged_rating, &caller_label)
        .await;

    // -- Verify the outcome against the worker's TBA $LH. -------------------
    colony_report_outcome(
        accept,
        bounty_id,
        &worker,
        reward_wei,
        judged_rating,
        min_accept,
        &caller_label,
    )
    .await;
    // A reject is a NORMAL outcome (the colony rationally declined sub-quality
    // work), not an error — exit 0 on both branches.
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn parse_colony_run_parses_task_reward_worker_judge_ttl() {
        // Full form: multi-word task + reward + worker + judge + ttl, interleaved.
        let p = parse_colony_run_args(&args(&[
            "QA:", "probe", "one", "flow", "--reward", "0.02", "--worker", "vex-qa", "--judge",
            "claude", "--ttl", "1h",
        ]))
        .unwrap();
        assert_eq!(p.task, "QA: probe one flow");
        assert_eq!(p.reward_wei, 20_000_000_000_000_000); // 0.02 LH
        assert_eq!(p.worker.as_deref(), Some("vex-qa"));
        assert_eq!(p.judge.as_deref(), Some("claude"));
        assert_eq!(p.ttl_secs, 3600);

        // No worker, no judge, no ttl → all None, default ttl.
        let p = parse_colony_run_args(&args(&["fix the bug", "--reward", "1"])).unwrap();
        assert_eq!(p.task, "fix the bug");
        assert_eq!(p.reward_wei, 1_000_000_000_000_000_000); // 1 LH
        assert!(p.worker.is_none());
        assert!(p.judge.is_none());
        assert_eq!(p.ttl_secs, INVITE_DEFAULT_TTL_SECS);
    }

    #[test]
    fn parse_judge_rating_extracts_digit_and_rationale() {
        // The canonical shape: digit on line 1, rationale on line 2.
        let (r, why) = parse_judge_rating("5\nSpecific, correct, and on-topic.");
        assert_eq!(r, 5);
        assert_eq!(why, "Specific, correct, and on-topic.");

        // A bogus/hallucinated result the judge rejects.
        let (r, _) = parse_judge_rating("1\nFabricated — localharness has no control API.");
        assert_eq!(r, 1);

        // Chatty prefix: still finds the first 1..5 digit.
        let (r, _) = parse_judge_rating("Rating: 4 — good but slightly vague.");
        assert_eq!(r, 4);

        // Out-of-range / no digit → neutral default of 3.
        assert_eq!(parse_judge_rating("no number here at all").0, 3);
        // A leading 0/6..9 is skipped; the first IN-RANGE digit ON LINE 1 wins.
        assert_eq!(parse_judge_rating("0 then 2").0, 2);
        assert_eq!(parse_judge_rating("99999").0, 3);

        // REGRESSION (#89): a number in the rationale (a later line) must NOT
        // override the score. Here line 1 has no in-range digit, so the parse
        // defaults to 3 rather than grabbing the "2" from the rationale.
        let (r, _) = parse_judge_rating("Score: ten\nIt got 2 of the 3 checks wrong.");
        assert_eq!(r, 3);
        // And the genuine score on line 1 wins even when later lines have digits.
        let (r, _) = parse_judge_rating("4\nFails 1 of 5 edge cases noted in 2024.");
        assert_eq!(r, 4);
        // Leading blank lines are skipped to the first non-empty line.
        let (r, why) = parse_judge_rating("\n\n5\nExcellent.");
        assert_eq!(r, 5);
        assert_eq!(why, "Excellent.");
    }

    #[test]
    fn median_rating_aggregates_panel() {
        // Odd N → the true middle (sorted).
        assert_eq!(median_rating(&[5, 4, 5]), 5);
        assert_eq!(median_rating(&[1, 3, 5]), 3);
        assert_eq!(median_rating(&[2, 5, 4, 3, 1]), 3); // unsorted input is sorted
        // A single rogue judge can't swing the median.
        assert_eq!(median_rating(&[5, 5, 1]), 5);
        assert_eq!(median_rating(&[1, 1, 5]), 1);
        // Even N → the LOWER-MIDDLE (conservative: never inflate a split panel).
        assert_eq!(median_rating(&[4, 5]), 4);
        assert_eq!(median_rating(&[1, 2, 4, 5]), 2); // sorted [1,2,4,5], idx n/2-1 = 1 → 2
        // All-same → that value (any N).
        assert_eq!(median_rating(&[4, 4, 4]), 4);
        assert_eq!(median_rating(&[2, 2]), 2);
        // A single judge → its own rating (a `--judge X` panel of one).
        assert_eq!(median_rating(&[3]), 3);
        // EMPTY (every judge turn failed) → the neutral 3 default.
        assert_eq!(median_rating(&[]), 3);
        // The median of any 1..=5 inputs stays in range.
        assert!((1..=5).contains(&median_rating(&[1, 5])));
    }

    #[test]
    fn should_accept_gates_payment_on_the_rating_bar() {
        // Default bar (2): a median of 1 (clear failure / hallucination) is REJECTED;
        // 2..=5 are PAID. This is the core economic-rationality rule.
        assert!(!should_accept(1, COLONY_DEFAULT_MIN_ACCEPT)); // median 1 / min 2 → reject
        assert!(should_accept(2, COLONY_DEFAULT_MIN_ACCEPT)); // median 2 / min 2 → accept
        assert!(should_accept(3, COLONY_DEFAULT_MIN_ACCEPT));
        assert!(should_accept(5, COLONY_DEFAULT_MIN_ACCEPT));
        // Boundary is `>=`: equal accepts, one below rejects.
        assert!(should_accept(2, 2)); // median 2 / min 2 → accept
        assert!(should_accept(5, 5)); // median 5 / min 5 → accept
        assert!(!should_accept(4, 5)); // median 4 / min 5 → reject
        assert!(!should_accept(1, 2));
        // A strict bar of 5 only ever pays a unanimous 5★.
        assert!(!should_accept(4, 5));
        assert!(should_accept(5, 5));
        // A bar of 1 (the lowest valid floor) pays everything 1..=5.
        for m in 1..=5 {
            assert!(should_accept(m, 1));
        }
        // Clamp/edge: a stray 0 median can never sneak past a min-1 floor, and an
        // out-of-band min is pulled into 1..=5 so the comparison stays sane.
        assert!(should_accept(0, 1)); // 0 clamps up to 1 ≥ 1 → accept (floor case)
        assert!(!should_accept(0, 2)); // 0 clamps to 1 < 2 → reject
        assert!(should_accept(5, 0)); // min 0 clamps up to 1 → 5 ≥ 1 → accept
        assert!(should_accept(6, 5)); // 6 clamps to 5 ≥ 5 → accept
        assert!(should_accept(5, 9)); // min 9 clamps to 5 → 5 ≥ 5 → accept
    }

    #[test]
    fn parse_colony_run_args_min_accept_flag() {
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Default when omitted.
        let p = parse_colony_run_args(&mk(&["QA task", "--reward", "0.01"])).unwrap();
        assert_eq!(p.min_accept, COLONY_DEFAULT_MIN_ACCEPT);
        assert_eq!(p.min_accept, 2);
        // Explicit, in-range.
        let p =
            parse_colony_run_args(&mk(&["QA task", "--reward", "0.01", "--min-accept-rating", "5"]))
                .unwrap();
        assert_eq!(p.min_accept, 5);
        let p =
            parse_colony_run_args(&mk(&["QA task", "--reward", "0.01", "--min-accept-rating", "1"]))
                .unwrap();
        assert_eq!(p.min_accept, 1);
        // 0 and out-of-band / non-numeric are rejected at parse time.
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating", "0"])).is_err());
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating", "6"])).is_err());
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating", "x"])).is_err());
        // Dangling flag is an error.
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating"])).is_err());
    }

    #[test]
    fn select_judge_panel_excludes_worker_and_caller_distinct() {
        let local: Vec<String> = ["claude", "dex-qa", "vex-qa", "iris-qa", "juno-qa"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Worker = vex-qa, caller = claude → both excluded; first 3 of the rest.
        let panel = select_judge_panel(&local, "vex-qa", "claude", 3);
        assert_eq!(panel, vec!["dex-qa", "iris-qa", "juno-qa"]);
        assert!(!panel.iter().any(|n| n == "vex-qa" || n == "claude"));
        // Fewer neutral agents than asked → returns what's available (no panic).
        let small = vec!["claude".to_string(), "dex-qa".to_string()];
        let panel = select_judge_panel(&small, "dex-qa", "claude", 3);
        assert!(panel.is_empty()); // both excluded → no neutral agent
        let panel = select_judge_panel(&small, "someone-else", "claude", 3);
        assert_eq!(panel, vec!["dex-qa"]); // only one neutral remains
        // Distinct: a duplicate name in the input is taken once.
        let dupes = vec!["dex-qa".to_string(), "dex-qa".to_string(), "iris-qa".to_string()];
        let panel = select_judge_panel(&dupes, "w", "c", 5);
        assert_eq!(panel, vec!["dex-qa", "iris-qa"]);
        // N caps the size even when more neutral agents exist.
        let panel = select_judge_panel(&local, "w", "c", 2);
        assert_eq!(panel.len(), 2);
    }

    #[test]
    fn parse_colony_run_args_judges_flag() {
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Default panel size when --judges is omitted.
        let p = parse_colony_run_args(&mk(&["QA task", "--reward", "0.01"])).unwrap();
        assert_eq!(p.judges, COLONY_DEFAULT_PANEL);
        // Explicit --judges.
        let p = parse_colony_run_args(&mk(&["QA task", "--reward", "0.01", "--judges", "5"])).unwrap();
        assert_eq!(p.judges, 5);
        // Zero / non-numeric is rejected.
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--judges", "0"])).is_err());
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--judges", "x"])).is_err());
    }

    #[test]
    fn colony_judge_prompt_embeds_task_result_and_serverless_context() {
        let p = colony_judge_prompt("find a real security issue", "the control API binds 0.0.0.0");
        assert!(p.contains("find a real security issue"));
        assert!(p.contains("the control API binds 0.0.0.0"));
        // The accuracy anchor that lets the judge catch the serverless hallucination.
        assert!(p.contains("SERVERLESS"));
        assert!(p.contains("single digit 1-5"));
        // A non-code result injects NO compile-evidence block.
        assert!(!p.contains("GROUND-TRUTH COMPILE EVIDENCE"));
    }

    #[test]
    fn extract_rustlite_snippet_finds_backtick_fenced_and_bare_and_ignores_prose() {
        // single-backtick span
        let s = extract_rustlite_snippet("Repro:\n`fn main() { let x = 1; }`").unwrap();
        assert_eq!(s, "fn main() { let x = 1; }");
        // fenced block with a language tag
        let s = extract_rustlite_snippet("```rust\nfn frame(t: i32) -> i32 { t }\n```").unwrap();
        assert_eq!(s, "fn frame(t: i32) -> i32 { t }");
        // BARE (unquoted) line — the case the backtick-only extractor missed live.
        let s = extract_rustlite_snippet("Bug: crash!\nRepro:\nconst X: i32 = 999;").unwrap();
        assert_eq!(s, "const X: i32 = 999;");
        // prose with backticks but no code keyword → nothing code-like
        assert!(extract_rustlite_snippet("see the `config` value and `x`").is_none());
        assert!(extract_rustlite_snippet("just prose, no code at all").is_none());
    }

    #[test]
    fn rustlite_compile_evidence_grounds_the_judge() {
        // The phantom-`>>` finding: REJECTED at the first `<` (LH0100), contradicting
        // the worker's claimed `>>`-mislex mechanism.
        let ev = rustlite_compile_evidence(
            "Bug: naive `>>` lexing. Repro: `fn main() { let x: Option<Option<i32>> = None; }`",
        )
        .expect("a backticked snippet yields evidence");
        assert!(ev.contains("REJECTED"));
        assert!(ev.contains("LH0100"));
        // The phantom-CRASH finding as a BARE huge-literal const (the live #3 case):
        // a clean LH0005, NOT a crash — the "compiler panic" claim is refuted.
        let ev = rustlite_compile_evidence(
            "Bug: compiler panic on overflow.\nRepro:\nconst X: i32 = 999999999999999999999999999999999;",
        )
        .expect("a bare code line yields evidence");
        assert!(ev.contains("REJECTED"));
        assert!(ev.contains("LH0005"));
        assert!(!ev.contains("CRASHES"), "a clean coded error must not be reported as a crash");
        // A genuinely valid cartridge compiles cleanly → positive evidence.
        let ev = rustlite_compile_evidence("`#[no_mangle] fn frame(t: i32) -> i32 { t }`").unwrap();
        assert!(ev.contains("COMPILES cleanly"));
        // A valid bare statement is wrapped with an entry and compiles cleanly (it
        // must NOT read as a false rejection for lacking a frame/render export).
        let ev = rustlite_compile_evidence("Repro:\nlet a: i32 = 5;").unwrap();
        assert!(ev.contains("COMPILES cleanly"), "got: {ev}");
        // No repro → no evidence (judge behaves as before).
        assert!(rustlite_compile_evidence("no code here").is_none());
        // The evidence actually reaches the judge prompt.
        let p = colony_judge_prompt(
            "name a rustlite edge case",
            "Repro: `fn main() { let x: Option<i32> = None; }`",
        );
        assert!(p.contains("GROUND-TRUTH COMPILE EVIDENCE"));
    }

    #[test]
    fn pick_reputation_aware_blends_task_fit_then_reputation() {
        let cand = |name: &str, task_rank: usize, count: u64, sum: u64| WorkerCandidate {
            name: name.into(),
            task_rank,
            rep_count: count,
            rep_sum: sum,
        };

        // 1. PROVEN beats UNPROVEN at SIMILAR task fit (both within the band):
        //    dex-qa is the very top match but has no reputation; vex-qa is one rank
        //    behind but carries 5.0★ from 2 attestations → reputation decides.
        let set = [
            cand("dex-qa", 0, 0, 0),  // top task fit, unproven
            cand("vex-qa", 1, 2, 10), // similar task fit, 5.0★ x2
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "vex-qa");

        // 2. TASK FIT still DOMINATES a wildly-irrelevant high-rep agent: a 5.0★
        //    agent buried far down the discover list (way outside the band) loses
        //    to the relevant-but-unproven top match.
        let set = [
            cand("dex-qa", 0, 0, 0),     // top task fit, unproven
            cand("guru-bot", 50, 9, 45), // irrelevant to the task, 5.0★ x9
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "dex-qa");

        // 3. Higher AVERAGE wins within a tier (4.0★ x10 vs 5.0★ x2 → 5.0 wins).
        let set = [
            cand("steady", 0, 10, 40), // 4.0★
            cand("ace", 1, 2, 10),     // 5.0★
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "ace");

        // 4. Equal average → MORE attestations is the tiebreak (5.0 x3 > 5.0 x1).
        let set = [
            cand("rookie", 0, 1, 5),   // 5.0★ x1
            cand("veteran", 1, 3, 15), // 5.0★ x3
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "veteran");

        // 5. All unproven → falls back to best discover rank (deterministic).
        let set = [cand("first", 0, 0, 0), cand("second", 1, 0, 0)];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "first");

        // Empty candidate set → no pick.
        assert!(pick_reputation_aware(&[]).is_none());
    }

    #[test]
    fn colony_task_keywords_extracts_significant_words() {
        // The dogfood task: stop words + short words + punctuation dropped, the
        // meaningful keywords kept in order (so "qa" surfaces the QA fleet).
        let kw = colony_task_keywords("QA: suggest one concrete localharness CLI improvement (1-2 sentences)");
        assert!(kw.contains(&"localharness".to_string()));
        assert!(kw.contains(&"improvement".to_string()));
        assert!(kw.contains(&"concrete".to_string()));
        // "cli" is 3 chars and not a stop word → kept (punctuation stripped).
        assert!(kw.contains(&"cli".to_string()));
        // Stop words + sub-3-char tokens ("qa", "12") dropped; "suggest" is a stop word.
        assert!(!kw.contains(&"one".to_string()));
        assert!(!kw.contains(&"suggest".to_string()));
        assert!(!kw.contains(&"qa".to_string()));
        assert!(!kw.contains(&"12".to_string()));
        // No dupes, bounded.
        let dup = colony_task_keywords("test test test localharness localharness bounty");
        assert_eq!(dup.iter().filter(|w| *w == "test").count(), 1);
        assert!(dup.len() <= COLONY_MAX_KEYWORDS);
        // All-stop-word / empty task → no keywords.
        assert!(colony_task_keywords("the a an to of").is_empty());
        assert!(colony_task_keywords("").is_empty());
    }

    #[test]
    fn colony_pick_reasoning_is_transparent() {
        let proven = WorkerCandidate { name: "vex-qa".into(), task_rank: 0, rep_count: 2, rep_sum: 10 };
        let line = colony_pick_reasoning(&proven);
        assert!(line.contains("vex-qa"));
        assert!(line.contains("reputation 5.0"));
        assert!(line.contains("2 attestations"));
        assert!(line.contains("top task match"));

        let unproven = WorkerCandidate { name: "dex-qa".into(), task_rank: 1, rep_count: 0, rep_sum: 0 };
        let line = colony_pick_reasoning(&unproven);
        assert!(line.contains("dex-qa"));
        assert!(line.contains("no reputation yet"));
        assert!(line.contains("task match #2"));

        // Singular grammar for a single attestation.
        let single = WorkerCandidate { name: "solo".into(), task_rank: 0, rep_count: 1, rep_sum: 4 };
        let line = colony_pick_reasoning(&single);
        assert!(line.contains("4.0 from 1 attestation"));
        assert!(!line.contains("attestations"));
    }

    #[test]
    fn colony_recovery_hint_matches_the_working_command_per_status() {
        // OPEN (0): `bounty cancel` is the recovery — and it WORKS while Open.
        let h = colony_recovery_hint(7, "me", Some(0));
        assert!(h.contains("bounty cancel --as me 7"), "open → cancel: {h}");
        assert!(!h.contains("bounty reclaim"), "open must NOT steer to reclaim: {h}");

        // CLAIMED (1) / SUBMITTED (2): `cancelBounty` reverts `NotOpen`, so the
        // ONLY working recovery is the ttl-gated `bounty reclaim`. The earlier bug
        // headlined `bounty cancel` here, which always reverts mid-cycle.
        for st in [1u8, 2] {
            let h = colony_recovery_hint(7, "me", Some(st));
            assert!(h.contains("bounty reclaim --as me 7"), "status {st} → reclaim: {h}");
            // Must NOT headline the cancel command that would revert.
            assert!(
                !h.contains("bounty cancel --as me 7"),
                "status {st} must not advise the reverting cancel: {h}"
            );
        }

        // PAID (3): nothing to recover (the worker was paid).
        let h = colony_recovery_hint(7, "me", Some(3));
        assert!(h.to_lowercase().contains("paid"));
        assert!(!h.contains("bounty cancel") && !h.contains("bounty reclaim"));

        // Cancelled (4) / Reclaimed (5): already refunded, nothing to do.
        for st in [4u8, 5] {
            let h = colony_recovery_hint(7, "me", Some(st));
            assert!(h.to_lowercase().contains("refunded"), "status {st}: {h}");
        }

        // Unknown / unreadable status → surface BOTH so the user is never stuck.
        let h = colony_recovery_hint(7, "me", None);
        assert!(h.contains("bounty cancel --as me 7"));
        assert!(h.contains("bounty reclaim --as me 7"));
    }

    #[test]
    fn parse_colony_run_rejects_bad_forms() {
        assert!(parse_colony_run_args(&args(&[])).is_err()); // empty
        assert!(parse_colony_run_args(&args(&["task"])).is_err()); // no --reward
        assert!(parse_colony_run_args(&args(&["task", "--reward", "0"])).is_err()); // zero reward
        assert!(parse_colony_run_args(&args(&["--reward", "1"])).is_err()); // no task
        assert!(parse_colony_run_args(&args(&["task", "--reward"])).is_err()); // dangling --reward
        assert!(parse_colony_run_args(&args(&["t", "--reward", "1", "--worker"])).is_err()); // dangling
    }

    #[test]
    fn is_transient_rpc_error_classifies_hiccups_not_reverts() {
        // The live failure mode: a decode/transport hiccup on the response.
        assert!(is_transient_rpc_error(
            "eth_sendRawTransaction decode: error decoding response body"
        ));
        assert!(is_transient_rpc_error("connection reset"));
        assert!(is_transient_rpc_error("request timed out"));
        // A real contract revert must NOT be retried (it'll just revert again).
        assert!(!is_transient_rpc_error("execution reverted: NotOpen()"));
        assert!(!is_transient_rpc_error("revert: bounty not submitted"));
        assert!(!is_transient_rpc_error("insufficient balance"));
    }

    #[test]
    fn work_result_needs_retry_only_on_empty_or_transient() {
        // FIX 1: the WORK turn retries ONCE on an empty reply OR a transient error.
        let ok = |s: &str| -> Result<(String, Option<Vec<u8>>), String> { Ok((s.to_string(), None)) };
        let err = |s: &str| -> Result<(String, Option<Vec<u8>>), String> { Err(s.to_string()) };
        // Empty / whitespace-only reply (the tick-14 hiccup) → retry.
        assert!(work_result_needs_retry(&ok("")));
        assert!(work_result_needs_retry(&ok("   \n\t  ")));
        // A NON-empty result is a real deliverable → NEVER retry.
        assert!(!work_result_needs_retry(&ok("here is the answer")));
        assert!(!work_result_needs_retry(&ok("  trimmed but present  ")));
        // A TRANSIENT failure → retry (reuses is_transient_rpc_error detection).
        assert!(work_result_needs_retry(&err("error decoding response body")));
        assert!(work_result_needs_retry(&err("connection reset")));
        assert!(work_result_needs_retry(&err("request timed out")));
        // A GENUINE (non-transient) failure → bail, do NOT retry.
        assert!(!work_result_needs_retry(&err("402 payment required — redeem a code")));
        assert!(!work_result_needs_retry(&err("execution reverted: NotOpen()")));
        assert!(!work_result_needs_retry(&err("is not a registered agent")));
    }

    #[test]
    fn exclude_caller_candidates_removes_the_caller_and_can_empty() {
        // FIX 2: the auto-PICK pool must EXCLUDE the caller (self-deal). Pure test
        // of the exclusion rule — no network.
        let cand = |name: &str, rank: usize| WorkerCandidate {
            name: name.into(),
            task_rank: rank,
            rep_count: 0,
            rep_sum: 0,
        };
        // The caller is removed; the rest survive in order.
        let pool = vec![cand("claude", 0), cand("dex-qa", 1), cand("vex-qa", 2)];
        let kept = exclude_caller_candidates(pool, "claude");
        assert_eq!(kept.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["dex-qa", "vex-qa"]);
        assert!(!kept.iter().any(|c| c.name == "claude"));
        // Case-insensitive match on the bare name (subdomain names are case-insens).
        let pool = vec![cand("Claude", 0), cand("dex-qa", 1)];
        let kept = exclude_caller_candidates(pool, "claude");
        assert_eq!(kept.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["dex-qa"]);
        // Excluding the caller can EMPTY the pool → the auto-PICK then fails with
        // "no valid worker" (pick_reputation_aware(&[]) is None).
        let pool = vec![cand("claude", 0)];
        let kept = exclude_caller_candidates(pool, "claude");
        assert!(kept.is_empty());
        assert!(pick_reputation_aware(&kept).is_none());
        // A non-caller pool is untouched.
        let pool = vec![cand("dex-qa", 0), cand("vex-qa", 1)];
        let kept = exclude_caller_candidates(pool, "claude");
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn judge_equals_worker_guards_the_explicit_override() {
        // FIX 3: --judge naming the WORKER is rejected (self-inflated rating); a
        // neutral judge is allowed.
        assert!(judge_equals_worker("vex-qa", "vex-qa")); // exact self-judge → reject
        assert!(judge_equals_worker("VEX-QA", "vex-qa")); // case-insensitive
        assert!(judge_equals_worker("vex-qa", "VEX-QA"));
        // A different (neutral) agent is fine.
        assert!(!judge_equals_worker("dex-qa", "vex-qa"));
        assert!(!judge_equals_worker("claude", "vex-qa"));
    }
}
