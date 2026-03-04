//! Integration tests for `mantle_core::conflict`.
//!
//! Tests exercise the full public API:
//!   `build_conflict_map`, `ConflictMap`, `ModRole`, `ConflictSummary`
//!
//! All tests are pure in-memory — no filesystem or Steam dependency.

use mantle_core::conflict::{
    build_conflict_map,
    resolution::{conflict_summary_for_mod, ModRole},
    ModEntry,
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn entry(id: &str, files: &[&str]) -> ModEntry {
    ModEntry {
        id: id.to_owned(),
        files: files.iter().map(|&s| s.to_owned()).collect(),
    }
}

// ── clean cases ───────────────────────────────────────────────────────────────

#[test]
fn empty_input_produces_clean_map() {
    let map = build_conflict_map(&[]);
    assert!(map.is_clean());
    assert_eq!(map.total_file_conflicts(), 0);
}

#[test]
fn single_mod_always_clean() {
    let map = build_conflict_map(&[entry(
        "a",
        &[
            "data/a.esp",
            "data/meshes/armor.nif",
            "data/textures/armor_d.dds",
        ],
    )]);
    assert!(map.is_clean());
}

#[test]
fn multiple_mods_no_overlap_are_clean() {
    let map = build_conflict_map(&[
        entry("a", &["data/a.esp"]),
        entry("b", &["data/b.esp"]),
        entry("c", &["data/c.esp", "data/c_mesh.nif"]),
    ]);
    assert!(map.is_clean());
}

// ── conflict detection ────────────────────────────────────────────────────────

#[test]
fn simple_two_mod_conflict() {
    let map = build_conflict_map(&[
        entry("high", &["data/shared.esp", "data/only_high.nif"]),
        entry("low", &["data/shared.esp", "data/only_low.dds"]),
    ]);

    // Exactly one conflict.
    assert_eq!(map.total_file_conflicts(), 1);

    let e = map.entry_for_path("data/shared.esp").unwrap();
    assert_eq!(e.winner, "high");
    assert_eq!(e.losers, ["low"]);

    // Uncontested paths absent from map.
    assert!(map.entry_for_path("data/only_high.nif").is_none());
    assert!(map.entry_for_path("data/only_low.dds").is_none());
}

#[test]
fn priority_index_zero_always_wins() {
    // Regardless of name, the mod at index 0 must win.
    let map = build_conflict_map(&[
        entry("zzz_last_alphabetically", &["data/shared.esp"]),
        entry("aaa_first_alphabetically", &["data/shared.esp"]),
    ]);
    let e = map.entry_for_path("data/shared.esp").unwrap();
    assert_eq!(
        e.winner, "zzz_last_alphabetically",
        "index 0 must win regardless of name ordering"
    );
}

#[test]
fn three_way_conflict_one_winner_two_losers() {
    let map = build_conflict_map(&[
        entry("a", &["data/x.esp"]),
        entry("b", &["data/x.esp"]),
        entry("c", &["data/x.esp"]),
    ]);
    let e = map.entry_for_path("data/x.esp").unwrap();
    assert_eq!(e.winner, "a");
    let mut losers = e.losers.clone();
    losers.sort();
    assert_eq!(losers, ["b", "c"]);
}

#[test]
fn mod_is_winner_on_some_paths_loser_on_others() {
    // "mid" wins over "low" on y.nif, loses to "high" on x.esp.
    let map = build_conflict_map(&[
        entry("high", &["data/x.esp"]),
        entry("mid", &["data/x.esp", "data/y.nif"]),
        entry("low", &["data/y.nif"]),
    ]);
    assert_eq!(map.total_file_conflicts(), 2);
    assert_eq!(map.entry_for_path("data/x.esp").unwrap().winner, "high");
    assert_eq!(map.entry_for_path("data/y.nif").unwrap().winner, "mid");
}

// ── ConflictMap query methods ─────────────────────────────────────────────────

#[test]
fn win_and_loss_counts_are_correct() {
    let map = build_conflict_map(&[
        entry("a", &["data/x.esp", "data/y.nif"]),
        entry("b", &["data/x.esp", "data/b_only.dds"]),
        entry("c", &["data/y.nif", "data/c_only.esp"]),
    ]);
    assert_eq!(map.win_count_for_mod("a"), 2);
    assert_eq!(map.loss_count_for_mod("a"), 0);
    assert_eq!(map.win_count_for_mod("b"), 0);
    assert_eq!(map.loss_count_for_mod("b"), 1);
    assert_eq!(map.win_count_for_mod("c"), 0);
    assert_eq!(map.loss_count_for_mod("c"), 1);
}

#[test]
fn conflicts_for_mod_iterator_correct_count() {
    let map = build_conflict_map(&[
        entry("a", &["data/x.esp", "data/y.nif", "data/z.dds"]),
        entry("b", &["data/x.esp", "data/y.nif"]),
        entry("c", &["data/z.dds"]),
    ]);
    // "a" wins all three paths.
    assert_eq!(map.conflicts_for_mod("a").count(), 3);
    // "b" loses on x.esp and y.nif.
    assert_eq!(map.conflicts_for_mod("b").count(), 2);
    // "c" loses on z.dds.
    assert_eq!(map.conflicts_for_mod("c").count(), 1);
}

#[test]
fn conflicted_paths_iterator_yields_all_contested() {
    let map = build_conflict_map(&[
        entry("a", &["data/p1.esp", "data/p2.nif"]),
        entry("b", &["data/p1.esp", "data/p2.nif"]),
    ]);
    let mut paths: Vec<&str> = map.conflicted_paths().collect();
    paths.sort();
    assert_eq!(paths, ["data/p1.esp", "data/p2.nif"]);
}

// ── ModRole ───────────────────────────────────────────────────────────────────

#[test]
fn role_clean_when_no_conflicts() {
    let map = build_conflict_map(&[entry("solo", &["data/only.esp"])]);
    assert_eq!(map.role_of_mod("solo"), ModRole::Clean);
    assert_eq!(map.role_of_mod("nonexistent"), ModRole::Clean);
}

#[test]
fn role_winner_when_wins_and_no_losses() {
    let map = build_conflict_map(&[
        entry("winner", &["data/x.esp"]),
        entry("loser", &["data/x.esp"]),
    ]);
    assert_eq!(map.role_of_mod("winner"), ModRole::Winner);
}

#[test]
fn role_loser_when_losses_and_no_wins() {
    let map = build_conflict_map(&[
        entry("winner", &["data/x.esp"]),
        entry("loser", &["data/x.esp"]),
    ]);
    assert_eq!(map.role_of_mod("loser"), ModRole::Loser);
}

#[test]
fn role_both_when_wins_and_losses() {
    let map = build_conflict_map(&[
        entry("high", &["data/x.esp"]),
        entry("mid", &["data/x.esp", "data/y.nif"]),
        entry("low", &["data/y.nif"]),
    ]);
    assert_eq!(map.role_of_mod("mid"), ModRole::Both);
}

// ── ConflictSummary ───────────────────────────────────────────────────────────

#[test]
fn summary_for_winner() {
    let map = build_conflict_map(&[
        entry("a", &["data/x.esp", "data/y.nif"]),
        entry("b", &["data/x.esp"]),
        entry("c", &["data/y.nif"]),
    ]);
    let s = conflict_summary_for_mod(&map, "a");
    assert_eq!(s.wins, 2);
    assert_eq!(s.losses, 0);
    assert_eq!(s.role, ModRole::Winner);
    assert!(!s.is_clean());
}

#[test]
fn summary_for_clean_mod() {
    let map = build_conflict_map(&[entry("clean", &["data/unique.nif"])]);
    let s = conflict_summary_for_mod(&map, "clean");
    assert_eq!(s.wins, 0);
    assert_eq!(s.losses, 0);
    assert!(s.is_clean());
}

// ── realistic scenario ────────────────────────────────────────────────────────

/// Simulate a realistic 5-mod load order with several overlapping files.
/// Asserts the conflict semantics match VFS priority order exactly.
#[test]
fn realistic_five_mod_scenario() {
    // Priority: USSEP > SkyUI > ImmersiveArmors > WICO > RandomMod
    let map = build_conflict_map(&[
        entry("USSEP", &["data/skyrim.esm", "data/update.esm"]),
        entry("SkyUI", &["data/skyui_se.esp", "data/interface/skyui/mainskyui.swf"]),
        entry(
            "ImmersiveArmors",
            &[
                "data/hothtrooper44_armory_extravaganza.esp",
                "data/meshes/armor/iron/cuirass_1.nif",
            ],
        ),
        entry(
            "WICO",
            &[
                "data/wico - immersive people.esp",
                "data/meshes/armor/iron/cuirass_1.nif",
            ],
        ), // conflicts with IA
        entry(
            "RandomMod",
            &[
                "data/skyrim.esm", // conflicts with USSEP
                "data/skyui_se.esp",
            ],
        ), // conflicts with SkyUI
    ]);

    // Two files are contested: skyrim.esm and skyui_se.esp and cuirass_1.nif
    assert_eq!(map.total_file_conflicts(), 3);

    assert_eq!(map.entry_for_path("data/skyrim.esm").unwrap().winner, "USSEP");
    assert_eq!(map.entry_for_path("data/skyui_se.esp").unwrap().winner, "SkyUI");
    assert_eq!(
        map.entry_for_path("data/meshes/armor/iron/cuirass_1.nif").unwrap().winner,
        "ImmersiveArmors"
    );

    // USSEP and SkyUI and IA are all pure winners.
    assert_eq!(map.role_of_mod("USSEP"), ModRole::Winner);
    assert_eq!(map.role_of_mod("SkyUI"), ModRole::Winner);
    assert_eq!(map.role_of_mod("ImmersiveArmors"), ModRole::Winner);
    // WICO loses its one contested file.
    assert_eq!(map.role_of_mod("WICO"), ModRole::Loser);
    // RandomMod loses both its contested files.
    assert_eq!(map.role_of_mod("RandomMod"), ModRole::Loser);
}
