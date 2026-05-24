//! Tenant detection from `window.location.hostname`.
//!
//! Mirrors the logic of `self.tools`' `extractSubdomain` middleware
//! but runs client-side in wasm — Next-style middleware isn't needed
//! because we serve a single static bundle for every host. Per-origin
//! browser sandboxing already gives us per-subdomain OPFS / storage
//! isolation for free; this module's only job is to *name* which
//! tenant we are so the chrome can display it and (eventually) so the
//! app can look up registry state for it.

const ROOT_DOMAIN: &str = "localharness.xyz";

/// What kind of host we're currently being served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Host {
    /// `localharness.xyz` (or `www.`). Marketing / signup home.
    Apex,
    /// `<name>.localharness.xyz`. A user-owned space.
    Tenant(String),
    /// `localhost`, a Vercel preview URL, or anything else we don't
    /// recognise. Treat as a developer / generic context.
    Other(String),
}

impl Host {
    /// Short display label for the chrome.
    #[allow(dead_code)]
    pub(crate) fn label(&self) -> String {
        match self {
            Host::Apex => format!("{ROOT_DOMAIN} · home"),
            Host::Tenant(name) => format!("{name}.{ROOT_DOMAIN}"),
            Host::Other(h) => h.clone(),
        }
    }

    /// The tenant slug, if any. `None` for apex / unknown hosts.
    #[allow(dead_code)]
    pub(crate) fn tenant(&self) -> Option<&str> {
        match self {
            Host::Tenant(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Read `window.location.hostname` and classify it. Defaults to
/// `Host::Other("unknown")` if the browser refuses to hand it over
/// (won't happen in practice).
pub(crate) fn current() -> Host {
    let hostname = web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .unwrap_or_else(|| "unknown".into());
    classify(&hostname)
}

/// Normalise a user-typed subdomain candidate to the same character
/// set the on-chain registry enforces: lowercase ASCII alphanumeric +
/// dash. Mirrors the `[^a-z0-9-]` filter the contract applies before
/// minting.
pub(crate) fn sanitize(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

fn classify(hostname: &str) -> Host {
    // Strip any leading "www.".
    let h = hostname.strip_prefix("www.").unwrap_or(hostname);

    if h == ROOT_DOMAIN {
        return Host::Apex;
    }

    // Vercel preview URLs look like `antig-abc123-compusophys-projects.vercel.app`.
    // Treat those as Other so the chrome shows the raw hostname rather
    // than pretending it's a tenant. (self.tools handles a similar case
    // with the `---` preview pattern.)
    if h.ends_with(".vercel.app") || h == "localhost" || h.ends_with(".localhost") {
        return Host::Other(hostname.to_string());
    }

    // `<sub>.localharness.xyz` — a single-label tenant prefix.
    if let Some(prefix) = h.strip_suffix(&format!(".{ROOT_DOMAIN}")) {
        if !prefix.is_empty() && !prefix.contains('.') {
            return Host::Tenant(prefix.to_string());
        }
    }

    Host::Other(hostname.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apex_is_apex() {
        assert_eq!(classify("localharness.xyz"), Host::Apex);
        assert_eq!(classify("www.localharness.xyz"), Host::Apex);
    }

    #[test]
    fn tenant_extracts_prefix() {
        assert_eq!(classify("john.localharness.xyz"), Host::Tenant("john".into()));
        assert_eq!(classify("foo-bar.localharness.xyz"), Host::Tenant("foo-bar".into()));
    }

    #[test]
    fn multi_label_subdomain_is_other() {
        // We only support single-label tenant prefixes for now.
        assert!(matches!(
            classify("a.b.localharness.xyz"),
            Host::Other(_)
        ));
    }

    #[test]
    fn vercel_preview_is_other() {
        assert!(matches!(
            classify("antig-abc-compusophys-projects.vercel.app"),
            Host::Other(_)
        ));
    }

    #[test]
    fn localhost_is_other() {
        assert!(matches!(classify("localhost"), Host::Other(_)));
        assert!(matches!(classify("john.localhost"), Host::Other(_)));
    }

    #[test]
    fn tenant_method_returns_slug() {
        assert_eq!(Host::Tenant("john".into()).tenant(), Some("john"));
        assert_eq!(Host::Apex.tenant(), None);
    }
}
