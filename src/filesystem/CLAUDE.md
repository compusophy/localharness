# src/filesystem — Filesystem trait + impls subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/filesystem/`).
> The 8 fs builtins call `crate::filesystem::Filesystem`, NOT `tokio::fs` — so they
> run on wasm/OPFS too. `SharedFilesystem = Arc<dyn Filesystem>`. Surface:
> `read · write_atomic · metadata · read_dir · walk · delete · rename`.

## Impls
- **`NativeFilesystem`** (`native.rs`, `feature=native`): tokio::fs + walkdir +
  tempfile; atomic write = tempfile + rename.
- **`OpfsFilesystem`** (`opfs.rs`, wasm32): OPFS via web-sys; atomic =
  `FileSystemWritableFileStream.close()` swap.
- **`EncryptedFilesystem`** (`encrypted.rs`, all targets): seed-keyed AES-256-GCM
  AT REST over any inner impl — see the hard rule below.
- **`RootedFilesystem`** (`rooted.rs`): confine ops to a sub-tree — the bashlite
  CLI sandbox.
`GeminiConnectionStrategy::connect` honors a caller-supplied FS via
`with_filesystem`, else auto-installs `NativeFilesystem` on native (None on wasm —
the caller supplies OPFS).

## ⛔ HARD RULE: EncryptedFilesystem must NEVER encrypt the EXEMPT_FILES
Format: `LHE1‖nonce‖ct`; read sniffs the magic → decrypt (tamper = clear error),
else legacy plaintext passes through FOREVER (so enabling encryption never bricks
existing plaintext). Key tag `localharness/v0/opfs-at-rest` is PINNED — don't change
it (it'd orphan every encrypted file). Installed over OPFS by
`wallet_store::{load,create_and_persist,import}`; seedless origins stay plaintext.

The pinned `EXEMPT_FILES` are NEVER encrypted, and this is load-bearing:
- **`.lh_wallet`** — the seed IS the key root; sealing it = unrecoverable identity
  brick (you'd need the key to read the key). This is THE one that bricks everyone.
- **pre-wallet boot files** — `.lh_owner` / `.lh_linked_owner` / `.lh_device_key`
  (read BEFORE the wallet/key exists).
- the **2 model artifacts** (Gemma weights).
If you add a file that must be readable before the seed is loaded, add it to
`EXEMPT_FILES` or it bricks boot. Never "just encrypt everything."

## wasm: this is one of the few subsystems that runs identically on native + wasm
(the builtins gate on a supplied `Filesystem`, not `feature=native`). Keep new
methods cfg-clean — guard `fs_builtins_gate_on_filesystem_not_native`.
