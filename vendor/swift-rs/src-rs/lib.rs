//! Call Swift functions from Rust with ease!
#![cfg_attr(docsrs, feature(doc_cfg))]
// Polaris: local addition, not upstream. Cargo caps lints for registry crates but not
// for a `[patch]` path source, so `-D warnings` would judge this third-party code by
// whatever the newest toolchain warns about. Restore the cap that 1.0.7 from crates.io had.
#![allow(warnings)]

mod autorelease;
mod swift;
mod swift_arg;
mod swift_ret;
mod types;

pub use autorelease::*;
pub use swift::*;
pub use swift_arg::*;
pub use swift_ret::*;
pub use types::*;

#[cfg(feature = "build")]
#[cfg_attr(docsrs, doc(cfg(feature = "build")))]
mod build;
#[cfg(feature = "build")]
pub use build::*;
