// Mod Watcher — example Rhai plugin for Mantle Manager
//
// Announces when mods are installed, enabled, or disabled, and logs a summary
// whenever the conflict map is updated.
//
// What this demonstrates:
//   - Subscribing to multiple event types in a single init()
//   - Reading mod info fields from the event map (mod.name, mod.slug, etc.)
//   - Subscribing to "All" events and dispatching by event["type"]
//   - Keeping a running counter in a captured variable across handler calls
//   - Using ctx.mod_list() to read the current mod list
//   - The ConflictMapUpdated event payload (total_conflicts, affected_mods)

// ─── Plugin lifecycle ────────────────────────────────────────────────────────

fn init(ctx) {
    // A counter shared across multiple closures.
    // In Rhai, closures capture by reference, so both handlers share this variable.
    let install_count = 0;

    // ── Subscribe to ModInstalled ────────────────────────────────────────────
    ctx.subscribe("ModInstalled", |event| {
        let mod_info = event["mod"];
        let name     = mod_info["name"];
        let version  = mod_info["version"];
        install_count += 1;
        ctx.notify("Info", `Mod installed: ${name} v${version} (total this session: ${install_count})`);
    });

    // ── Subscribe to ModEnabled ──────────────────────────────────────────────
    ctx.subscribe("ModEnabled", |event| {
        let mod_info = event["mod"];
        let name     = mod_info["name"];
        let priority = mod_info["priority"];
        ctx.notify("Info", `Mod enabled: ${name} (priority ${priority})`);
    });

    // ── Subscribe to ModDisabled ─────────────────────────────────────────────
    ctx.subscribe("ModDisabled", |event| {
        let mod_info = event["mod"];
        let name     = mod_info["name"];
        ctx.notify("Info", `Mod disabled: ${name}`);
    });

    // ── Subscribe to ConflictMapUpdated ──────────────────────────────────────
    // Warns only when at least one conflict exists.
    ctx.subscribe("ConflictMapUpdated", |event| {
        let total    = event["total_conflicts"];
        let affected = event["affected_mods"];   // array of slug strings

        if total == 0 {
            ctx.notify("Info", "Conflict map updated — no conflicts.");
            return;
        }

        // Build a comma-separated list of affected mod slugs.
        let slug_list = "";
        let i = 0;
        while i < affected.len() {
            if i > 0 { slug_list += ", "; }
            slug_list += affected[i];
            i += 1;
        }

        ctx.notify("Warning", `${total} conflict(s) in: ${slug_list}`);
    });

    // Print the current mod list to the log on startup (demonstrates ctx.mod_list()).
    let mods = ctx.mod_list();
    print(`[mod_watcher] loaded — ${mods.len()} mods in active profile`);

    ctx.notify("Info", "Mod Watcher loaded.");
}

// shutdown() — subscriptions are cleared automatically; nothing extra needed.
fn shutdown() {}
