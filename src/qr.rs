//! Inline SVG QR-code generation for the browser app's share surfaces —
//! device pairing, the post-publish share fragment, and the `?invite=`
//! share link. Hoisted out of `app::templates` (the `turn_flow` /
//! `confirm` native-testable-core pattern) so the encode → SVG pipeline
//! has a unit test that runs under a native `cargo test` — the `app`
//! module itself is wasm32-only, so a helper trapped inside it can never
//! be tested off-browser.
//!
//! Pure compute: the `qrcode` crate with `default-features = false,
//! features = ["svg"]` pulls ZERO transitive deps and compiles on every
//! target. Gated on `browser-app` (the only consumer) so default native
//! builds don't pay for it.

// Only the wasm32 `app` module calls this at runtime; a native
// `--features browser-app` build compiles it solely for the unit test.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// Encode `data` as an inline SVG QR code: black modules on a WHITE
/// background (phone cameras need the light quiet zone even on a dark
/// screen), monochrome, `shape-rendering="crispEdges"` stamped by the
/// renderer so modules never blur. Returned as a raw SVG string for
/// `PreEscaped` injection — fits the no-canvas, innerHTML-swap
/// architecture. `None` on the (practically impossible) encode failure
/// so callers still render the typeable link as a fallback.
pub(crate) fn qr_svg(data: &str) -> Option<String> {
    use qrcode::render::svg;
    use qrcode::QrCode;

    let code = QrCode::new(data.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(200, 200)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::qr_svg;

    /// A known share link must encode to a non-empty inline SVG with the
    /// deterministic viewBox for its QR version (49 bytes → version 4:
    /// 33 modules + 2×4 quiet zone = 41; scaled ×5 to clear the 200px
    /// minimum → 205), crisp edges, and white module background.
    #[test]
    fn invite_link_renders_svg_qr() {
        let link = "https://localharness.xyz/?invite=inv-1-abcdefghij";
        let svg = qr_svg(link).expect("QR encode must succeed for a share link");
        assert!(!svg.is_empty());
        assert!(svg.contains("<svg"), "must be inline SVG: {svg}");
        assert!(
            svg.contains(r#"viewBox="0 0 205 205""#),
            "expected the version-4 41-module ×5 viewBox: {svg}"
        );
        assert!(
            svg.contains(r#"shape-rendering="crispEdges""#),
            "modules must not anti-alias: {svg}"
        );
        assert!(svg.contains("#ffffff"), "white module background: {svg}");
    }
}
