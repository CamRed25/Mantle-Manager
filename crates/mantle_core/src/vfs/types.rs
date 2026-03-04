//! Shared types used by all VFS backend implementations.

use std::path::PathBuf;

/// Parameters required to mount a virtual overlay.
///
/// Passed to each backend's `mount()` function. The semantics are the same
/// regardless of which backend is active — backends adapt internally.
///
/// # Priority ordering
/// `lower_dirs[0]` is the **highest-priority** mod directory. Files present in
/// `lower_dirs[0]` take precedence over the same path in any later entry.
/// This matches the overlayfs `lowerdir` convention: leftmost wins.
///
/// # Example
/// ```
/// use mantle_core::vfs::types::MountParams;
/// use std::path::PathBuf;
///
/// let params = MountParams {
///     lower_dirs: vec![
///         PathBuf::from("/mods/patch"),     // highest priority — wins conflicts
///         PathBuf::from("/mods/base-mod"),  // lower priority
///     ],
///     merge_dir: PathBuf::from("/tmp/mantle/merge"),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct MountParams {
    /// Mod source directories, index 0 = highest priority.
    ///
    /// Each entry must be an absolute path to a readable directory containing
    /// the mod's extracted data files (e.g. `Data/` subtree).
    pub lower_dirs: Vec<PathBuf>,

    /// Directory where the merged view is presented to the game process.
    ///
    /// Created by the backend if it does not exist. Must be an absolute path
    /// on a writable filesystem.
    pub merge_dir: PathBuf,
}
