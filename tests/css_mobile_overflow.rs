//! On-chain #81 contract: long hex hashes / raw JSON must WRAP in the
//! transcript, never pan it sideways on a phone. Guards the three
//! declarations a puppeteer probe at 411x826 proved necessary; if one is
//! dropped, the mobile grid overflows again.

fn rule(css: &str, selector: &str) -> String {
    let start = css
        .find(selector)
        .unwrap_or_else(|| panic!("selector missing from styles.css: {selector}"));
    let open = css[start..].find('{').expect("rule opens") + start;
    let close = css[open..].find('}').expect("rule closes") + open;
    css[open..close].to_string()
}

#[test]
fn transcript_never_scrolls_horizontally() {
    let css = std::fs::read_to_string("web/styles.css").expect("web/styles.css");
    // The structural guarantee: the transcript scrollport clips X.
    assert!(rule(&css, "main.layout .transcript {").contains("overflow-x: hidden"));
    // The wrapping rules that keep content inside it.
    assert!(rule(&css, ".system-status {").contains("overflow-wrap: anywhere"));
    assert!(rule(&css, ".confirm-callout {").contains("overflow-wrap: anywhere"));
    let pre = rule(&css, ".turn .body pre {");
    assert!(pre.contains("white-space: pre-wrap") && pre.contains("word-break: break-word"));
}
