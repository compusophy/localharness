//! Gas budgets for sponsored on-chain writes.
//!
//! The canonical `setMetadata` formula lives in `registry::set_metadata_gas`
//! (the CLI budgets from it too); this module just re-exports it for the
//! app-side call sites.

pub(crate) use crate::registry::set_metadata_gas;
