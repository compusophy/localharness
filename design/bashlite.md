# bashlite — a tiny sandboxed shell that scripts the platform's tools

**STATUS: v1 + part of v2/v3 SHIPPED** (2026-06-19). Origin: platform-builder ask
— "integrate the WASI terminal with our tools/filesystems, almost bash-like, a
scripting abstraction: bashlite."

**Shipped:** the v1 core (`src/bashlite/`, lexer/parser/eval, 45 native tests) +
`execute_script` browser tool. Plus the FRACTAL + composition layer: a `run`/
`source` builtin that executes another `.bl` in a nested evaluator (shared fs +
fuel, bounded recursion) so a script is a composition of scripts; `for f in $(…)`
field-splitting (fan-out); the dead `BashHost::run_builtin` extension seam is now
WIRED, and READ-ONLY `lh-*` platform commands ship in `src/bashlite/platform.rs`
(`lh-whoami`/`lh-balance`/`lh-resolve`/`lh-price` — localharnesslite). End-to-end:
`localharness sh <script.bl> [--as <name>] [--confirm]` runs a script on the
native fs (`src/filesystem/rooted.rs` confines the sandbox to the script's dir)
with the `lh-*` commands — proven live (`examples/bashlite/fractal.bl` composes 3
levels of scripts + resolves a mainnet agent). **Value-MOVING `lh-send` ships
behind the dry-run-manifest gate** (`platform::dispatch_write`): the script runs
DRY first (each move emits a one-line plan, nothing sent), the host collects the
manifest, and `--confirm` re-runs LIVE — proven live with
`examples/bashlite/treasury.bl` (read balance → plan a send → refuse without
--confirm). **`lh-publish <name> <source.rl>` SHIPPED** (behind the same
dry-run-manifest gate): compile a rustlite cartridge + publish/UPDATE it as an
OWNED subdomain's public face (sponsored setMetadata; refuses unregistered /
foreign names), plus the read-only `lh-list-mine` (the caller's owned names, one
per line) — so `for s in $(lh-list-mine); do lh-publish $s app.rl; done` updates
many apps from one shell. The owner's seed signs for every owned NFT, so this is
composable scripting, NOT an actor/message model. **Remaining:** more
value-moving `lh-*` (`lh-create`); scheduler runs `.bl` (zero-LLM cron);
`lh-http` over `/api/fetch`.

## Why (the cost unlock)

Today an agent that does a multi-step chore runs a **tool-in-a-loop**: one tool
call per step, each a full LLM round that re-sends the entire context **+ all
~70 tool schemas** as input (see `design/architecture-analysis.md` + the cost
deep-dive). Four steps ≈ four rounds ≈ ~4× the input-token bill.

bashlite makes it **script-in-a-loop**: the agent gets ONE tool —
`execute_script(source)` — writes a single multi-line script, the platform runs
it locally in one sandboxed pass, and only the final output returns to the
model. Four steps → **one** LLM round. A flat ~75% input-token cut on
tool-heavy turns, stacking on top of prompt caching + difficulty routing
(the other two cost levers). This is the highest-leverage cost item after
caching, and it's a capability gain (agents express intent as a program, not a
stutter of round-trips).

## It builds on what already shipped (don't reinvent)

- **#6 WASI runtime** (`web/wasi-worker.js`, `run_wasm_cli`, `src/app/cli.rs`):
  a `wasi_snapshot_preview1` subset host + an off-main-thread worker + a terminal
  surface + a main-thread watchdog. bashlite RUNS on this — the terminal is its
  REPL, the watchdog is its timeout/fuel backstop.
- **`Filesystem` trait** (`src/filesystem/`): OPFS in the browser, Native on the
  CLI, Encrypted at rest. bashlite's `ls/cd/cat/write/grep/find/rm/mv` map
  straight onto it — same sandbox the fs builtins already use.
- **Platform tool logic** (`src/app/chat/tools/*`, `src/registry/*`): `lh-send`,
  `lh-shared-get/set`, `lh-subdomain create`, `lh-balance`, … are thin shims over
  tool/registry functions that already exist + are tested.
- **Native-testable-core pattern** (`rustlite/`, `raster.rs`, `compose.rs`): the
  interpreter is a pure Rust core over a host trait, so the lexer/parser/eval run
  under `cargo test` and the browser/CLI/scheduler just supply the host.

## Shape

A *tiny, deterministic, linear* shell — NOT full bash. `src/bashlite/`
(lexer → parser → eval) over a `BashHost` trait:

```
trait BashHost {            // supplied per surface (browser/CLI/scheduler)
    fs: &dyn Filesystem,    // ls/cd/cat/write/grep/find/rm/mv
    fn platform(cmd, args) -> Result<String>;  // lh-* commands
    fn confirm(action) -> Confirmed;            // value-moving gate
}
```

Language surface (kept small + total): variables (`x=$(...)`), pipes (`|`),
`if`/`for`/`while`, `[ ... ]` tests, `echo`, command substitution, exit codes.
No subshells-spawning-processes, no eval, no network except via `lh-http`.

```sh
cd /src
n=$(ls | grep ".rl" | wc -l)
echo "$n cartridges"
status=$(lh-shared-get deploy_status)
if [ "$status" != "active" ]; then
    lh-subdomain create worker-1
    lh-send worker-1 2.5          # value-moving → confirm-gated (below)
    lh-shared-set deploy_status active
fi
```

## The agent tool

`execute_script(source)` — registered in `chat/tools`. Runs the script through
the bashlite core with a `BashHost` bound to this tenant's OPFS + platform
identity, fuel/timeout-bounded (reuse the worker watchdog), returns
`{ exit_code, stdout, stderr }`. Read-only fs/platform reads run unattended; the
model gets the final stdout, not each step.

## Safety (the load-bearing design question)

Value-moving host commands (`lh-send`, `lh-spend`, `lh-subdomain create`, …) must
NOT execute silently inside a script. Options to settle in v2 design:
- **Dry-run + manifest:** a script with value-moving commands first runs in
  dry-run, emitting a manifest of the moves; the typed-confirmation gate
  (`confirm_guard`, the bordered callout) confirms the WHOLE manifest once; the
  real run is then authorized. Cleaner than pausing mid-script.
- Per-command confirm (rejected: a script with 3 sends = 3 callouts = the
  redundancy we just fixed).
Read-only + create-only-with-own-funds commands stay unattended. Same fuel +
watchdog + size caps as cartridges (untrusted-input posture).

## The scheduler payoff (zero-LLM automation)

Today a scheduled job (`ScheduleFacet` + the Vercel cron worker) drives the LLM
every tick. A bashlite job stores a SCRIPT on-chain; the cron worker runs the
script locally — **zero LLM calls for routine automation**. Recurring chores
(rebalance, poll, fan-out a fixed pipeline) cost gas + $LH only, no inference.
This alone justifies the build for any heavy scheduler user.

## Phased plan

- **v1** — `src/bashlite/` core (lexer/parser/eval, native tests) + fs commands
  over the `Filesystem` trait + `execute_script` tool (read/create only, no
  value moves) + run in the #6 terminal. Pure win, no new risk surface.
- **v2** — platform host commands (`lh-*`) + the dry-run-manifest confirm flow
  for value-moving ops. `.bl` files runnable from the file explorer.
- **v3** — scheduler runs bashlite scripts directly (zero-LLM cron); a `lh-http`
  command over the existing `/api/fetch` proxy; pipe into `compile_rustlite` so a
  script can build + publish a cartridge.

## Honest risks

A scripting language is real surface area: parser edge cases, non-termination
(→ fuel/timeout mandatory), and sandbox-escape if a host command is
under-scoped. Keep the language tiny and total; every host command inherits the
existing tool's gating (allowlist + confirm). Don't grow toward POSIX — the
value is the *integration*, not bash compatibility.

## Relationship to the other execution surfaces

Three complementary layers, one sandbox: **rustlite** builds visual apps
(cartridges), the **WASI runtime** runs compiled CLIs, **bashlite** scripts the
tools + orchestrates the platform. bashlite is the glue that makes the agent's
intent a *program* instead of a token-expensive stutter of round-trips.
