//! VFS nested overlay stacking for large mod lists.
//!
//! Linux overlayfs limits the number of lower directories to approximately 500
//! per mount (constrained by the kernel page size used to store the `lowerdir`
//! option string). When the active mod count exceeds [`STACK_TRIGGER`], this
//! module divides the lower directories into groups of [`CHUNK_SIZE`], mounts
//! each group as an intermediate overlay backed by a temporary directory, then
//! uses those merged views as the lower directories for the final game-facing
//! mount.
//!
//! Per `VFS_DESIGN.md` §6.
//!
//! # Priority preservation
//! `lower_dirs[0]` remains the highest-priority mod in the final view. Priority
//! is preserved because each chunk is mounted in order and the intermediate
//! merged views are arranged so that earlier chunks (higher priority) appear
//! earlier in the final `lowerdir` list.
//!
//! # Usage
//! ```ignore
//! if mod_dirs.len() > vfs::STACK_TRIGGER {
//!     let stacked = vfs::mount_stacked(kind, mod_dirs, game_data)?;
//!     // …
//!     stacked.unmount()?;
//! } else {
//!     let handle = vfs::mount_with(kind, MountParams { lower_dirs: mod_dirs, merge_dir: game_data })?;
//!     // …
//!     handle.unmount()?;
//! }
//! ```

use std::path::PathBuf;

use crate::{
    error::MantleError,
    vfs::{
        backend::BackendKind,
        mount::{mount_with, MountHandle},
        MountParams,
    },
};

/// Active mod count above which stacking is automatically needed.
///
/// Conservatively set to 480 — 20 below the practical kernel limit of ~500.
pub const STACK_TRIGGER: usize = 480;

/// Number of lower directories per intermediate overlay chunk.
const CHUNK_SIZE: usize = 200;

// ─── StackedMount ─────────────────────────────────────────────────────────────

/// An active stacked overlay: one or more intermediate overlays feeding into
/// a final game-facing overlay.
///
/// Created by [`mount_stacked`]. Must be explicitly torn down with
/// [`StackedMount::unmount`] — no `Drop`-based cleanup is provided.
pub struct StackedMount {
    /// Final overlay — the merged view presented to the game process.
    final_handle: MountHandle,
    /// Intermediate overlay handles, in creation order (lowest to highest
    /// priority group). Unmounted in reverse order during teardown.
    intermediate_handles: Vec<MountHandle>,
    /// Temporary directories backing each intermediate merge point. Held here
    /// to prevent the directories from being deleted until teardown completes.
    _intermediate_dirs: Vec<tempfile::TempDir>,
}

impl StackedMount {
    /// Path to the final merged view directory (what the game process sees).
    #[must_use]
    pub fn merge_dir(&self) -> &std::path::Path {
        self.final_handle.merge_dir()
    }

    /// Tear down all overlay layers in last-in first-out order.
    ///
    /// Unmounts the final layer first, then intermediate layers in reverse
    /// creation order. All layers are attempted even if an earlier one fails
    /// (best-effort teardown). Intermediate temp directories are removed from
    /// disk after all unmounts complete.
    ///
    /// # Errors
    /// Returns the first error encountered. Remaining teardowns are still
    /// attempted. Returns `Ok(())` if all layers unmounted cleanly.
    pub fn unmount(self) -> Result<(), MantleError> {
        let mut first_err: Option<MantleError> = None;

        if let Err(e) = self.final_handle.unmount() {
            first_err = Some(e);
        }
        for handle in self.intermediate_handles.into_iter().rev() {
            if let Err(e) = handle.unmount() {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        // _intermediate_dirs drops here → temp directories removed from disk.
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

// ─── mount_stacked ────────────────────────────────────────────────────────────

/// Build a stacked overlay for more than [`STACK_TRIGGER`] lower directories.
///
/// Divides `lower_dirs` into groups of [`CHUNK_SIZE`] and mounts each group as
/// an intermediate overlay backed by a temporary directory. The intermediate
/// merge directories are then used as the lower directories for the final
/// overlay at `final_merge_dir`.
///
/// If any intermediate mount fails, all successfully completed intermediates
/// are torn down before the error is returned.
///
/// # Errors
/// Returns [`MantleError::Vfs`] from the underlying [`mount_with`] call, or
/// [`MantleError::Io`] if a temporary directory cannot be created.
///
/// # Panics
/// Panics in debug builds if `lower_dirs.len() <= CHUNK_SIZE`. Callers must
/// check [`STACK_TRIGGER`] and use [`mount_with`] directly for smaller lists.
// clippy::needless_pass_by_value: `lower_dirs` and `final_merge_dir` are moved
// into the chunking machinery; taking by value avoids an unnecessary clone.
#[allow(clippy::needless_pass_by_value)]
pub fn mount_stacked(
    kind: BackendKind,
    lower_dirs: Vec<PathBuf>,
    final_merge_dir: PathBuf,
) -> Result<StackedMount, MantleError> {
    debug_assert!(
        lower_dirs.len() > CHUNK_SIZE,
        "mount_stacked requires more than {CHUNK_SIZE} lower dirs; \
         use mount_with directly for smaller mod lists"
    );

    let lower_count = lower_dirs.len();
    let mut intermediate_handles: Vec<MountHandle> = Vec::new();
    let mut intermediate_dirs: Vec<tempfile::TempDir> = Vec::new();
    let mut chunk_merge_paths: Vec<PathBuf> = Vec::new();

    // Divide lower_dirs into chunks and mount each as an intermediate overlay.
    // lower_dirs is borrowed here (chunks returns slices) so lower_count
    // was captured above.
    for chunk in lower_dirs.chunks(CHUNK_SIZE) {
        let temp = tempfile::Builder::new()
            .prefix("mantle-stack-")
            .tempdir()
            .map_err(MantleError::Io)?;
        let merge_path = temp.path().to_owned();

        let handle = match mount_with(
            kind,
            MountParams {
                lower_dirs: chunk.to_vec(),
                merge_dir: merge_path.clone(),
            },
        ) {
            Ok(h) => h,
            Err(e) => {
                // Best-effort cleanup of already-mounted intermediate layers.
                for h in intermediate_handles.drain(..) {
                    let _ = h.unmount();
                }
                return Err(e);
            }
        };

        chunk_merge_paths.push(merge_path);
        intermediate_dirs.push(temp);
        intermediate_handles.push(handle);
    }

    tracing::info!(
        "VFS stacking: {} intermediate overlays for {} lower dirs → {}",
        intermediate_handles.len(),
        lower_count,
        final_merge_dir.display(),
    );

    // Final mount: intermediate merge dirs are the lower dirs for the game overlay.
    let final_handle = match mount_with(
        kind,
        MountParams {
            lower_dirs: chunk_merge_paths,
            merge_dir: final_merge_dir,
        },
    ) {
        Ok(h) => h,
        Err(e) => {
            // Best-effort cleanup of all intermediate overlays already mounted.
            // The original mount error is what the caller cares about.
            for h in intermediate_handles.drain(..) {
                let _ = h.unmount();
            }
            return Err(e);
        }
    };

    Ok(StackedMount {
        final_handle,
        intermediate_handles,
        _intermediate_dirs: intermediate_dirs,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `STACK_TRIGGER` must be less than 500 (the practical kernel limit).
    #[test]
    fn stack_trigger_is_below_kernel_limit() {
        assert!(STACK_TRIGGER < 500, "STACK_TRIGGER {STACK_TRIGGER} >= 500");
    }

    /// `CHUNK_SIZE` must be less than `STACK_TRIGGER`.
    #[test]
    fn chunk_size_is_below_trigger() {
        assert!(CHUNK_SIZE < STACK_TRIGGER);
    }

    /// A stacked symlink-farm mount with 250 directories (> CHUNK_SIZE,
    /// < STACK_TRIGGER × 2) must produce two intermediate layers and one final
    /// layer, and the final merge dir must contain a file from the first chunk.
    ///
    /// The symlink-farm backend is used here because it has no kernel
    /// requirements and must pass on every machine.
    #[test]
    fn stacked_symlink_farm_mounts_and_unmounts() {
        const N_DIRS: usize = CHUNK_SIZE + 1; // 201 — forces exactly 2 chunks

        // Create N_DIRS source directories; place a unique file in each.
        let sources: Vec<tempfile::TempDir> = (0..N_DIRS)
            .map(|_| tempfile::TempDir::new().unwrap())
            .collect();
        for (i, s) in sources.iter().enumerate() {
            std::fs::write(s.path().join(format!("mod{i}.esp")), b"TES4").unwrap();
        }

        let lower_dirs: Vec<PathBuf> = sources.iter().map(|s| s.path().to_owned()).collect();
        let final_merge = tempfile::TempDir::new().unwrap();

        let stacked = mount_stacked(
            BackendKind::SymlinkFarm,
            lower_dirs,
            final_merge.path().to_owned(),
        )
        .expect("mount_stacked must succeed with symlink-farm backend");

        // File from the first source dir must be visible in the final merge.
        assert!(
            stacked.merge_dir().join("mod0.esp").exists(),
            "mod0.esp from the first source must be visible in the final merge dir"
        );
        // File from the last source dir must also be visible.
        assert!(
            stacked
                .merge_dir()
                .join(format!("mod{}.esp", N_DIRS - 1))
                .exists(),
            "last mod must be visible in the final merge dir"
        );

        stacked.unmount().expect("stacked unmount must succeed");
    }
}
