//! Read the existing RDS manager through its keyless CLI. RDS owns endpoint
//! identity, grants, sessions and transport. NDS never opens its secret keys.

pub mod view;
pub use view::*;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod native;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use native::Rds;

/// Canonical compiled-in runtime manifest for this integration.
pub const MANIFEST: &str = include_str!("../contracts/module.json");
