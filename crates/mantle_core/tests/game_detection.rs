//! Integration tests for game detection.
//!
//! Tests that require an actual Steam installation are guarded with a runtime
//! skip (per TESTING_GUIDE.md §5) — they print "SKIP: Steam not installed"
//! and pass rather than fail on machines without Steam.
//!
//! Tests that exercise pure filesystem logic (mock game directories) run
//! everywhere.

use mantle_core::game::{
    self, games,
    proton::{is_prefix_initialised, proton_prefix_in_dir},
    steam::detect_game_at_path,
    GameKind,
};
use std::path::PathBuf;
use tempfile::TempDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a fake game install directory with the given file(s) present.
fn fake_install(files: &[&str]) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    for f in files {
        let path = dir.path().join(f);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"stub").unwrap();
    }
    dir
}

// ── Pure-filesystem tests (no Steam required) ─────────────────────────────────

/// Happy path: detect_game_at_path returns a valid GameInfo.
#[test]
fn detect_skyrim_se_at_fake_path() {
    let def = games::by_slug("skyrim_se").unwrap();
    let install = fake_install(&[def.executable]);

    let info = detect_game_at_path(install.path(), def, None)
        .expect("must detect Skyrim SE when executable present");

    assert_eq!(info.slug, "skyrim_se");
    assert_eq!(info.kind, GameKind::SkyrimSE);
    assert_eq!(info.steam_app_id, 489830);
    assert_eq!(info.data_path, install.path().join("Data"));
    assert!(!info.is_proton());
}

#[test]
fn detect_morrowind_at_fake_path_uses_data_files_dir() {
    let def = games::by_slug("morrowind").unwrap();
    let install = fake_install(&[def.executable]);

    let info = detect_game_at_path(install.path(), def, None).unwrap();

    assert_eq!(info.slug, "morrowind");
    assert_eq!(info.kind, GameKind::Morrowind);
    assert_eq!(info.data_path, install.path().join("Data Files"));
}

#[test]
fn detect_returns_none_when_exe_missing() {
    let def = games::by_slug("fallout4").unwrap();
    let empty_dir = TempDir::new().unwrap();
    assert!(detect_game_at_path(empty_dir.path(), def, None).is_none());
}

#[test]
fn detect_attaches_proton_prefix() {
    let def = games::by_slug("fallout4").unwrap();
    let install = fake_install(&[def.executable]);
    let pfx = PathBuf::from("/tmp/fake/pfx");

    let info = detect_game_at_path(install.path(), def, Some(pfx.clone())).unwrap();

    assert_eq!(info.proton_prefix, Some(pfx));
    assert!(info.is_proton());
    assert_eq!(info.wine_prefix(), info.proton_prefix.as_deref());
}

/// Every game in KNOWN_GAMES can be detected when we create a fake install.
#[test]
fn every_known_game_detectable_with_fake_install() {
    for def in games::KNOWN_GAMES {
        let install = fake_install(&[def.executable]);
        let info = detect_game_at_path(install.path(), def, None);
        assert!(
            info.is_some(),
            "detect_game_at_path returned None for '{}' — \
             is the executable field correct? ('{}')",
            def.slug,
            def.executable
        );
        let info = info.unwrap();
        assert_eq!(info.slug, def.slug);
        assert_eq!(info.steam_app_id, def.steam_app_id);
    }
}

// ── Proton prefix tests (no Steam required) ───────────────────────────────────

#[test]
fn proton_prefix_in_dir_found_and_initialised() {
    let steam_root = TempDir::new().unwrap();
    let pfx = steam_root.path().join("steamapps/compatdata/489830/pfx");
    std::fs::create_dir_all(&pfx).unwrap();
    std::fs::write(pfx.join("user.reg"), b"W").unwrap();
    std::fs::write(pfx.join("system.reg"), b"W").unwrap();

    let found = proton_prefix_in_dir(steam_root.path(), 489830).unwrap();
    assert!(is_prefix_initialised(&found));
}

#[test]
fn proton_prefix_absent_when_compatdata_missing() {
    let root = TempDir::new().unwrap();
    assert!(proton_prefix_in_dir(root.path(), 489830).is_none());
}

// ── Steam-dependent tests (skip if Steam not installed) ───────────────────────

/// Scans the real Steam installation (if present) and asserts that any
/// detected games have valid paths and non-empty slugs.
#[test]
fn steam_scan_produces_valid_game_infos_if_steam_present() {
    let result = game::detect_all_steam();
    assert!(result.is_ok(), "detect_all_steam must not return Err: {:?}", result);

    let games_found = result.unwrap();

    if games_found.is_empty() {
        // No supported games installed — or Steam not present. Either is fine.
        return;
    }

    for g in &games_found {
        assert!(!g.slug.is_empty(), "game slug must not be empty: {g:?}");
        assert!(!g.name.is_empty(), "game name must not be empty: {g:?}");
        assert!(
            g.install_path.is_dir(),
            "install_path must be an existing directory for '{}': {}",
            g.slug,
            g.install_path.display()
        );
        assert!(g.steam_app_id > 0, "steam_app_id must be non-zero for '{}'", g.slug);
    }
}

/// If a game is found, its data_path must start with install_path.
#[test]
fn data_path_is_under_install_path() {
    let Ok(games_found) = game::detect_all_steam() else {
        return;
    };

    for g in &games_found {
        assert!(
            g.data_path.starts_with(&g.install_path),
            "data_path {} must be under install_path {} for '{}'",
            g.data_path.display(),
            g.install_path.display(),
            g.slug
        );
    }
}

/// If a Proton prefix is attached, the prefix directory must exist.
#[test]
fn proton_prefix_paths_exist_when_attached() {
    let Ok(games_found) = game::detect_all_steam() else {
        return;
    };

    for g in &games_found {
        if let Some(ref pfx) = g.proton_prefix {
            assert!(
                pfx.is_dir(),
                "proton_prefix {} must be an existing directory for '{}'",
                pfx.display(),
                g.slug
            );
        }
    }
}
