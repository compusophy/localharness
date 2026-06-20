//! Design-system tokens — the Rust source of truth for the browser app's
//! visual language. The owner edits the design system HERE (Rust consts),
//! not by hand-tweaking CSS.
//!
//! [`root_tokens_css`] emits a `:root { … }` block from these consts;
//! [`super::mount`] injects it into `<head>` (as `<style id="lh-tokens">`)
//! ahead of the static `web/styles.css`. Because CSS custom properties
//! resolve at *use* time, declaration order doesn't matter — the structural
//! / component rules in `styles.css` read `var(--bg)` etc. and pick up
//! whatever this block declares.
//!
//! Scope today: TOKENS only. The structural rules still live in
//! `styles.css` (a full CSS-in-Rust port of every component rule is a large
//! follow-up, deliberately NOT attempted here). The win this delivers: a
//! token change is a Rust edit, and the values can't silently drift from a
//! magic number buried in the stylesheet.
//!
//! ZERO visual regression: every value below is copied verbatim from the
//! `:root` block that previously lived at the top of `styles.css`, plus a
//! small named scale lifted from numbers that were already repeated
//! throughout the file (the spacing steps, the z-index layers).

// ---- Palette (monochrome brutalist) ------------------------------------
/// App background.
pub const BG: &str = "#080808";
/// Raised panel surface (cards, dialogs).
pub const PANEL: &str = "#0f0f0f";
/// Second panel tone.
pub const PANEL_2: &str = "#161616";
/// Hairline border colour.
pub const BORDER: &str = "#1e1e1e";
/// Primary foreground text.
pub const FG: &str = "#c8c8c8";
/// Muted / secondary text.
pub const MUTED: &str = "#555";
/// Accent (pure white) — emphasis, focus rings, active chips.
pub const ACCENT: &str = "#fff";
/// User-input text tone.
pub const USER: &str = "#777";
/// Error / danger tone.
pub const ERROR: &str = "#a05050";

// ---- Light theme (inverted monochrome) ---------------------------------
// The SAME brutalist language, flipped: near-white surfaces, near-black ink,
// black accent. Applied under `html.theme-light` (the toggle / `?theme=light`);
// only the palette + color-scheme flip — type, spacing, z-layers are shared.
pub const LIGHT_BG: &str = "#f6f6f6";
pub const LIGHT_PANEL: &str = "#ededed";
pub const LIGHT_PANEL_2: &str = "#e3e3e3";
pub const LIGHT_BORDER: &str = "#d6d6d6";
pub const LIGHT_FG: &str = "#1a1a1a";
pub const LIGHT_MUTED: &str = "#8a8a8a";
pub const LIGHT_ACCENT: &str = "#000";
pub const LIGHT_USER: &str = "#666";
pub const LIGHT_ERROR: &str = "#8a3a3a";

// ---- Typography --------------------------------------------------------
/// The one font stack — IBM Plex Mono with monospace fallbacks. Referenced
/// by `--font-mono`; the historical hardcoded copies in `styles.css` now
/// read this token.
pub const FONT_MONO: &str =
    "'IBM Plex Mono', ui-monospace, Menlo, Consolas, monospace";

// ---- Chrome ------------------------------------------------------------
/// THE uniform chrome margin — the spacing around the header's admin
/// button (above / below / right of it). Every piece of app chrome (header
/// padding, footer/terminal padding, transcript gutters) derives from this
/// ONE constant so header and footer geometry can never drift. Preserve
/// exactly — recently fixed.
pub const CHROME_PAD: &str = "16px";

// ---- Spacing scale -----------------------------------------------------
// A small named scale lifted from the values already repeated across the
// stylesheet. Used only where it removes real duplication; not every gap
// is forced onto the scale.
/// 4px — tightest rhythm (terminal body gap, fine row gaps).
pub const SPACE_1: &str = "4px";
/// 8px — default small gap (button rows, modal close offsets).
pub const SPACE_2: &str = "8px";
/// 12px — turn indent, transcript inter-turn gap, dialog row gaps.
pub const SPACE_3: &str = "12px";
/// 20px — modal/overlay backdrop padding, dialog inner padding.
pub const SPACE_4: &str = "20px";

// ---- Z-index layers ----------------------------------------------------
// The overlay stack, lowest → highest. Previously scattered as bare
// numbers; named so the ordering is legible at a glance.
/// Sticky site header.
pub const Z_HEADER: &str = "30";
/// Files modal + admin panel backdrop.
pub const Z_MODAL: &str = "100";
/// Brand dropdown menu.
pub const Z_MENU: &str = "120";
/// DISPLAY framebuffer overlay (above files, below api-key).
pub const Z_DISPLAY: &str = "140";
/// API-key modal.
pub const Z_API_KEY: &str = "150";

/// Emit the `:root { … }` design-token block. Injected into `<head>` at
/// mount; the static stylesheet's component rules consume these `var()`s.
pub(crate) fn root_tokens_css() -> String {
    format!(
        ":root {{\n\
         \x20\x20color-scheme: dark;\n\
         \x20\x20--bg: {BG};\n\
         \x20\x20--panel: {PANEL};\n\
         \x20\x20--panel-2: {PANEL_2};\n\
         \x20\x20--border: {BORDER};\n\
         \x20\x20--fg: {FG};\n\
         \x20\x20--muted: {MUTED};\n\
         \x20\x20--accent: {ACCENT};\n\
         \x20\x20--user: {USER};\n\
         \x20\x20--error: {ERROR};\n\
         \x20\x20--font-mono: {FONT_MONO};\n\
         \x20\x20--chrome-pad: {CHROME_PAD};\n\
         \x20\x20--space-1: {SPACE_1};\n\
         \x20\x20--space-2: {SPACE_2};\n\
         \x20\x20--space-3: {SPACE_3};\n\
         \x20\x20--space-4: {SPACE_4};\n\
         \x20\x20--z-header: {Z_HEADER};\n\
         \x20\x20--z-modal: {Z_MODAL};\n\
         \x20\x20--z-menu: {Z_MENU};\n\
         \x20\x20--z-display: {Z_DISPLAY};\n\
         \x20\x20--z-api-key: {Z_API_KEY};\n\
         }}\n\
         html.theme-light {{\n\
         \x20\x20color-scheme: light;\n\
         \x20\x20--bg: {LIGHT_BG};\n\
         \x20\x20--panel: {LIGHT_PANEL};\n\
         \x20\x20--panel-2: {LIGHT_PANEL_2};\n\
         \x20\x20--border: {LIGHT_BORDER};\n\
         \x20\x20--fg: {LIGHT_FG};\n\
         \x20\x20--muted: {LIGHT_MUTED};\n\
         \x20\x20--accent: {LIGHT_ACCENT};\n\
         \x20\x20--user: {LIGHT_USER};\n\
         \x20\x20--error: {LIGHT_ERROR};\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_tokens_css_declares_the_core_palette() {
        let css = root_tokens_css();
        assert!(css.starts_with(":root {"));
        assert!(css.trim_end().ends_with('}'));
        // Core tokens the static stylesheet depends on must be present.
        for needle in [
            "--bg: #080808",
            "--fg: #c8c8c8",
            "--border: #1e1e1e",
            "--accent: #fff",
            "--error: #a05050",
            "--chrome-pad: 16px",
            "color-scheme: dark",
        ] {
            assert!(css.contains(needle), "missing token: {needle}");
        }
    }

    #[test]
    fn root_tokens_css_includes_a_light_theme_override() {
        let css = root_tokens_css();
        assert!(css.contains("html.theme-light {"));
        assert!(css.contains("color-scheme: light"));
        assert!(css.contains(&format!("--bg: {LIGHT_BG}")));
        assert!(css.contains(&format!("--accent: {LIGHT_ACCENT}")));
    }
}
