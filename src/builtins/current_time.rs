//! `current_time` — the agent's built-in clock.
//!
//! Returns the current wall-clock time so the model never has to web_fetch an
//! external time API (on-chain feedback #45). Client-free and filesystem-free,
//! so it registers UNCONDITIONALLY on every backend and every target: native
//! reads `std::time::SystemTime`, wasm reads `js_sys::Date::now()`. The
//! ISO-8601 string is computed by hand (the crate has no chrono).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct CurrentTime;

/// Current UNIX time in milliseconds (cross-target).
fn now_millis() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
}

/// Format UNIX seconds as an ISO-8601 UTC instant (`YYYY-MM-DDTHH:MM:SSZ`),
/// dependency-free via the proleptic-Gregorian civil-from-days algorithm
/// (Howard Hinnant). Valid for any non-negative UNIX timestamp.
fn iso8601_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days: days since 1970-01-01 -> (y, m, d)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for CurrentTime {
    fn name(&self) -> &str {
        "current_time"
    }

    fn description(&self) -> &str {
        "Get the current wall-clock time from the host. Use this instead of \
         fetching an external time API. Returns the current UNIX timestamp \
         (seconds + milliseconds) and an ISO-8601 UTC string. Time is UTC; \
         there is no client time zone."
    }

    fn input_schema(&self) -> Value {
        // No arguments — an empty-object schema keeps the union-type lint
        // (single `type`, no additionalProperties) trivially satisfied.
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let millis = now_millis();
        let secs = millis / 1000;
        Ok(json!({
            "unix_seconds": secs,
            "unix_millis": millis,
            "iso8601_utc": iso8601_utc(secs),
            "timezone": "UTC"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_known_epochs() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        // leap-year day (2020-02-29)
        assert_eq!(iso8601_utc(1_582_934_400), "2020-02-29T00:00:00Z");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn execute_returns_consistent_secs_and_iso() {
        let v = CurrentTime.execute(json!({}), None).await.unwrap();
        let secs = v["unix_seconds"].as_u64().unwrap();
        assert!(secs > 1_700_000_000, "wall clock should be well past 2023");
        assert_eq!(v["iso8601_utc"], super::iso8601_utc(secs));
        assert_eq!(v["timezone"], "UTC");
    }
}
