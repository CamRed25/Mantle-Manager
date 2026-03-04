//! Mod installation pipeline — post-extraction processing steps.
//!
//! These operations run after a mod archive (zip / 7z / rar) is extracted into
//! the mods directory, preparing the loose files for use on Linux.
//!
//! | Module       | What it does                                               |
//! |--------------|-------------------------------------------------------------|
//! | [`case_fold`]| Rename all entries to lowercase — Linux FS compatibility.  |
//! | [`bsa`]      | Extract `.bsa` / `.ba2` archives inside mod directories.   |

pub mod bsa;
pub mod case_fold;

pub use bsa::{extract_mod_archives, find_bsa_archives, BsaExtractResult};
pub use case_fold::{normalize_dir, NormalizeResult};
