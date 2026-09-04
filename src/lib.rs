#[macro_use]
extern crate serde_derive;

mod config;
#[cfg(feature = "installer")]
mod disk_wrapper;
#[cfg(feature = "installer")]
mod installer;

/// Applications a person may decline while installing (E-OS `PR-016`/`PR-018`).
pub mod optional;
#[cfg(feature = "installer")]
pub use crate::installer::*;

pub use crate::config::file::FileConfig;
pub use crate::config::package::PackageConfig;
pub use crate::config::Config;
