//! Post-session diagnostics — checks that run after a game session ends.
//!
//! | Module       | What it does                                                     |
//! |--------------|------------------------------------------------------------------|
//! | [`cosave`]   | Detect saves missing script-extender cosaves (SKSE / xSE).      |
//! | [`overwrite`]| Classify newly generated files in the VFS upper directory.      |

pub mod cosave;
pub mod overwrite;

pub use cosave::{
    cosave_config_for, scan_missing_cosaves, se_is_installed, CosaveConfig, CosaveScanResult,
};
pub use overwrite::{
    scan_overwrite, scan_overwrite_with_categories, FileCategory, OverwriteScanResult, CATEGORIES,
};
