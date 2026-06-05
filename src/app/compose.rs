//! Compose mode — parses `?compose=foo,bar,baz` from the URL.
//!
//! The names are handed to the iframe-free `display::mount_composition`
//! compositor (roadmap Track A): each named subdomain's PUBLISHED `app.wasm` is
//! composited into one shared framebuffer, focus-gated and budget-capped. The
//! old embed-iframe grid this module used to paint (`paint_compose` +
//! `compose_chrome`) was removed when host::compose landed in the live app —
//! the "no iframes" rule and the whole point of Track A.

use super::dom;

/// `Some(names)` iff `?compose=...` is in the URL with at least one
/// comma-separated entry. Names are sanitized — only lowercase
/// alphanumerics + hyphen are allowed, matching the registry's name
/// charset. Empty entries silently dropped.
pub(crate) fn compose_names() -> Option<Vec<String>> {
    let window = dom::window().ok()?;
    let search = window.location().search().ok()?;
    let stripped = search.trim_start_matches('?');
    for pair in stripped.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        if k != "compose" {
            continue;
        }
        let decoded = super::decode_uri_component(v);
        let names: Vec<String> = decoded
            .split(',')
            .map(sanitize_name)
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            return None;
        }
        return Some(names);
    }
    None
}

fn sanitize_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}
