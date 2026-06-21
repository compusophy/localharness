//! `localharness link` — adopt a funded WEB wallet's seed into a TERMINAL
//! identity. The CLI mirror of the browser's QR seed-adoption (`?adopt=1#s=<ct>`).
//!
//! Phase 3 (section C-1, device-link) of `design/cli-mainnet-onboarding.md`. A
//! human funds an identity once in the browser, then links the CLI to the SAME
//! identity so it inherits the web wallet's `$LH` with no separate funding step.
//!
//! The transport is the EXISTING adopt ciphertext, reused byte-for-byte — no
//! new seed-transport scheme:
//!   - Desktop browser → admin → "add a device" prints a one-time CODE and an
//!     `?adopt=1#s=<hex>` URL/QR. The seed is sealed under
//!     [`wallet::adopt_code_key`] as `IV(12) || AES-256-GCM(ct||tag)`, hex into
//!     the fragment (`encryption::seal_with_raw_key`, the same AES-256-GCM the
//!     [`crate::filesystem::EncryptedFilesystem`] uses on native).
//!   - `localharness link --as <name> <adopt-url-or-ciphertext> --code <CODE>`
//!     derives the same key from the typed code, AES-GCM-decrypts the fragment
//!     to the 12-word mnemonic, validates it, derives the private key, and
//!     writes the perms-locked CLI key file. The terminal now IS that identity.
//!
//! Only the CLI-imports-a-browser-ciphertext direction ships here (the
//! verifiable half). The reverse — the CLI presenting a code/QR a browser
//! adopts — is deferred (the browser always generates its own code today).

use crate::{bytes_to_hex_str, fmt_lh, key_write_path, name_is_valid, registry, resolve_key_read_path, secure_key_file, wallet};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};


pub(crate) const LINK_USAGE: &str = "\
usage: localharness link --as <name> <adopt-url-or-ciphertext> [--code <CODE>]
  Adopt a FUNDED web wallet's seed into this terminal identity so the CLI
  inherits its $LH — the terminal mirror of the browser's QR seed-adoption.
  1. In the funded identity's browser (admin -> \"add a device\"), get the
     one-time CODE and the ?adopt=1#s=<ct> link/QR it shows.
  2. localharness link --as <name> '<that link>' --code <CODE>
     (the ciphertext alone, the bare #s=<hex>, or the full URL all work; the
      --code may instead be typed when prompted).
  Writes <name>'s key file (perms-locked); the CLI then acts as that identity.";

/// Maximum decrypted seed phrase size we will accept — a 24-word BIP-39
/// mnemonic is well under 256 bytes; anything larger is not a seed phrase and
/// is rejected before we touch the bip39 parser.
const MAX_PHRASE_BYTES: usize = 256;

/// Pull the adopt ciphertext (hex) out of whatever the user pasted: a full
/// `https://…/?adopt=1#s=<hex>` URL, a bare `#s=<hex>` / `s=<hex>` fragment, or
/// the raw hex itself. Returns the hex string (no `0x`/`s=` prefix), trimmed.
/// `None` when nothing hex-shaped is present.
pub(crate) fn extract_adopt_ciphertext(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    // Prefer the `s=` value after a fragment (`#…s=<hex>`) or query, so a full
    // URL or a copied `#s=<hex>` both resolve. Fall back to the raw input.
    let candidate = s
        .rsplit_once("s=")
        .map(|(_, v)| v)
        .unwrap_or(s)
        // A fragment value ends at the next param/whitespace boundary.
        .split(['&', ' ', '\t', '\n', '#'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    if candidate.is_empty() || candidate.len() % 2 != 0 {
        return None;
    }
    if !candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(candidate.to_string())
}

/// Decrypt an adopt ciphertext (`IV(12) || AES-256-GCM(ct||tag)`) under the
/// one-time `code` and return the recovered UTF-8 seed phrase. Mirrors the
/// browser's `encryption::open_with_raw_key` over [`wallet::adopt_code_key`]
/// EXACTLY — same key derivation, same AES-256-GCM, same `IV || ct+tag`
/// framing — so a browser-generated payload opens here. Pure + testable: a
/// wrong code, a tampered blob, or a non-phrase plaintext is a clear `Err`,
/// never silent garbage.
pub(crate) fn decrypt_adopt(ciphertext: &[u8], code: &str) -> Result<String, String> {
    const NONCE_LEN: usize = 12;
    const TAG_LEN: usize = 16;
    if ciphertext.len() < NONCE_LEN + TAG_LEN {
        return Err("ciphertext too short — not a valid adopt link".to_string());
    }
    let key = wallet::adopt_code_key(code);
    let cipher = Aes256Gcm::new((&key).into());
    let (nonce_bytes, ct) = ciphertext.split_at(NONCE_LEN);
    let mut nonce_arr = [0u8; NONCE_LEN];
    nonce_arr.copy_from_slice(nonce_bytes);
    let nonce = Nonce::from(nonce_arr);
    let plain = cipher
        .decrypt(&nonce, ct)
        .map_err(|_| "wrong code, or this is not an adopt link (GCM auth failed)".to_string())?;
    if plain.len() > MAX_PHRASE_BYTES {
        return Err("decrypted payload is not a seed phrase".to_string());
    }
    let phrase =
        String::from_utf8(plain).map_err(|_| "decrypted payload is not text".to_string())?;
    Ok(phrase)
}

/// `localharness link --as <name> <url-or-ct> [--code <CODE>]`. Decrypt a
/// browser-issued adopt payload and write `<name>`'s key file so the CLI acts
/// as that funded identity. Honors the same secure-key write as `create` /
/// `onboard` (config home, 0600, cwd-fallback gitignored) and NEVER overwrites
/// an existing key without confirmation.
pub(crate) async fn link(args: &[String]) -> i32 {
    let mut as_name: Option<String> = None;
    let mut code: Option<String> = None;
    let mut payload: Option<String> = None;
    let mut overwrite = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--as" => match args.get(i + 1) {
                Some(n) => {
                    as_name = Some(n.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--as needs a name\n{LINK_USAGE}");
                    return 2;
                }
            },
            "--code" => match args.get(i + 1) {
                Some(c) => {
                    code = Some(c.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--code needs a value\n{LINK_USAGE}");
                    return 2;
                }
            },
            "--force" => {
                overwrite = true;
                i += 1;
            }
            other if payload.is_none() => {
                payload = Some(other.to_string());
                i += 1;
            }
            other => {
                eprintln!("unexpected argument '{other}'\n{LINK_USAGE}");
                return 2;
            }
        }
    }

    let Some(name) = as_name else {
        eprintln!("link needs --as <name> to name the local key\n{LINK_USAGE}");
        return 2;
    };
    if !name_is_valid(&name) {
        eprintln!("invalid name '{name}' — use 1-63 chars of a-z, 0-9, hyphen");
        return 2;
    }
    // Refuse to clobber an existing identity key unless explicitly forced — the
    // key IS the identity (mirrors `onboard`/`create`'s reuse-don't-overwrite).
    if !overwrite {
        if let Some(existing) = resolve_key_read_path(&name) {
            eprintln!(
                "an identity key for '{name}' already exists at {existing} — \
                 pass --force to replace it with the linked seed"
            );
            return 1;
        }
    }

    let Some(payload) = payload else {
        eprintln!("link needs the ?adopt=1#s=<ct> link (or its ciphertext)\n{LINK_USAGE}");
        return 2;
    };
    let Some(ct_hex) = extract_adopt_ciphertext(&payload) else {
        eprintln!("could not find an adopt ciphertext in the input — paste the full ?adopt=1#s=<…> link, or the #s=<hex> fragment");
        return 2;
    };
    let ct = match localharness::encoding::hex_to_bytes(&ct_hex) {
        Ok(b) if !b.is_empty() => b,
        _ => {
            eprintln!("the adopt link's ciphertext is not valid hex");
            return 2;
        }
    };

    // The code: --code, else read it interactively (it's read off the desktop
    // screen, not embedded in the link, so the seed in browser history is
    // useless without it).
    let code = match code {
        Some(c) => c,
        None => match prompt_code() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}\n{LINK_USAGE}");
                return 2;
            }
        },
    };
    if code.trim().is_empty() {
        eprintln!("the one-time code is empty\n{LINK_USAGE}");
        return 2;
    }

    let phrase = match decrypt_adopt(&ct, &code) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("link failed: {e}");
            return 1;
        }
    };
    // Validate as a real BIP-39 mnemonic and derive the identity key from it
    // (the CLI key file stores private-key hex, like `create`/`onboard`).
    let mnemonic = match wallet::mnemonic_from_phrase(phrase.trim()) {
        Ok(m) => m,
        Err(_) => {
            eprintln!("link failed: the decrypted payload is not a valid seed phrase (wrong code?)");
            return 1;
        }
    };
    let signer = wallet::signer_from_mnemonic(&mnemonic);
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let priv_hex = format!("0x{}", localharness::encoding::bytes_to_hex(&signer.to_bytes()));

    let key_file = key_write_path(&name);
    if let Err(e) = std::fs::write(&key_file, format!("{priv_hex}\n")) {
        eprintln!("could not persist key to {key_file}: {e}");
        return 1;
    }
    secure_key_file(&key_file); // 0600 (unix) + keep a cwd-fallback key out of git.

    println!("✓ linked '{name}' to the web identity {addr}");
    println!("  key written to {key_file}");
    // Report the inherited balance so the user sees the funding carried over.
    let bal = registry::token_balance_of(&addr).await.unwrap_or(0);
    if bal > 0 {
        println!("  wallet balance: {} (inherited from the web wallet)", fmt_lh(bal));
    } else {
        println!("  the CLI now shares this identity's wallet + $LH");
    }
    println!("  try: localharness credits --as {name}");
    0
}

/// Read the one-time code from stdin (when `--code` was omitted). The code is
/// read off the desktop screen, so prompting keeps it out of shell history.
fn prompt_code() -> Result<String, String> {
    use std::io::{BufRead, Write};
    print!("one-time code (from the browser): ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("could not read the code: {e}"))?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_handles_url_fragment_and_raw_hex() {
        // Full adopt URL → just the hex.
        assert_eq!(
            extract_adopt_ciphertext("https://localharness.xyz/?adopt=1#s=deadbeef").as_deref(),
            Some("deadbeef")
        );
        // Bare fragment forms.
        assert_eq!(extract_adopt_ciphertext("#s=deadbeef").as_deref(), Some("deadbeef"));
        assert_eq!(extract_adopt_ciphertext("s=deadbeef").as_deref(), Some("deadbeef"));
        // Raw hex, with and without 0x.
        assert_eq!(extract_adopt_ciphertext("deadbeef").as_deref(), Some("deadbeef"));
        assert_eq!(extract_adopt_ciphertext("0xDEADBEEF").as_deref(), Some("DEADBEEF"));
        // Surrounding whitespace tolerated.
        assert_eq!(extract_adopt_ciphertext("  #s=deadbeef \n").as_deref(), Some("deadbeef"));
        // A trailing param after the fragment value is dropped.
        assert_eq!(
            extract_adopt_ciphertext("https://x/?adopt=1#s=deadbeef&foo=1").as_deref(),
            Some("deadbeef")
        );
    }

    #[test]
    fn extract_rejects_non_hex_and_empty() {
        assert_eq!(extract_adopt_ciphertext(""), None);
        assert_eq!(extract_adopt_ciphertext("   "), None);
        // Odd length is not byte-aligned hex.
        assert_eq!(extract_adopt_ciphertext("abc"), None);
        // Non-hex characters.
        assert_eq!(extract_adopt_ciphertext("#s=nothex!!"), None);
        assert_eq!(extract_adopt_ciphertext("hello world"), None);
    }

    /// THE cross-impl contract: a ciphertext built with the SAME key
    /// derivation + AES-256-GCM framing the browser uses
    /// (`IV(12) || ct+tag`, key = `wallet::adopt_code_key(code)`) decrypts
    /// back to the original phrase. This pins that `decrypt_adopt` is the
    /// faithful native twin of `encryption::open_with_raw_key`, so a
    /// browser-generated `?adopt=1#s=…` payload opens in the CLI.
    #[test]
    fn decrypt_round_trips_a_browser_style_sealed_phrase() {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};

        let code = "AB12CD";
        let phrase = "test test test test test test test test test test test junk";
        // Seal exactly like the browser: IV(12) || AES-256-GCM(ct||tag).
        let key = wallet::adopt_code_key(code);
        let cipher = Aes256Gcm::new((&key).into());
        let nonce_bytes = [9u8; 12];
        let nonce = Nonce::from(nonce_bytes);
        let ct = cipher.encrypt(&nonce, phrase.as_bytes()).unwrap();
        let mut sealed = Vec::new();
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ct);

        // The CLI opens it from just the code.
        assert_eq!(decrypt_adopt(&sealed, code).unwrap(), phrase);
        // Case-insensitivity of the code matches the browser normalization.
        assert_eq!(decrypt_adopt(&sealed, "ab12cd").unwrap(), phrase);
    }

    #[test]
    fn decrypt_rejects_wrong_code_and_short_input() {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};

        let key = wallet::adopt_code_key("RIGHT1");
        let cipher = Aes256Gcm::new((&key).into());
        let nonce_bytes = [3u8; 12];
        let ct = cipher.encrypt(&Nonce::from(nonce_bytes), b"hello".as_ref()).unwrap();
        let mut sealed = Vec::new();
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ct);

        // Wrong code → GCM auth failure, a clear error (never garbage).
        assert!(decrypt_adopt(&sealed, "WRONG1").is_err());
        // Too short to even hold a nonce + tag.
        assert!(decrypt_adopt(&[0u8; 10], "RIGHT1").is_err());
        // A tampered tag is rejected.
        let mut bad = sealed.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0x01;
        assert!(decrypt_adopt(&bad, "RIGHT1").is_err());
    }
}
