//! Read the existing RDS manager through its keyless CLI. The optional Linux
//! owned-screen adapter consumes Accounts-admitted native viewing inputs;
//! the existing NdsClient owns authority, purpose lifetime and presentation.

pub mod view;
pub use view::*;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod native;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use native::Rds;

/// Canonical compiled-in runtime manifest for this integration.
pub const MANIFEST: &str = include_str!("../contracts/module.json");

/// Native owned-view primitives; Accounts and the existing NdsClient own NDS
/// authority, purpose-key lifetime and presentation. Linux qualification only.
#[cfg(all(target_os = "linux", feature = "owned-screen"))]
pub mod owned;
