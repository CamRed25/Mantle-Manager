// Profile Greeter — example Rhai plugin for Mantle Manager
//
// Posts a notification whenever the active profile changes.
//
// What this demonstrates:
//   - Declaring init(ctx) and shutdown() — the two required script functions
//   - Using a closure to capture ctx so notify() is available inside the handler
//   - Subscribing to a single event type with ctx.subscribe()
//   - Accessing fields from the event map passed to the handler
//   - Reading and writing settings with ctx.get_setting() / ctx.set_setting()

// ─── Plugin lifecycle ────────────────────────────────────────────────────────

// init(ctx) is called once after the plugin is loaded.
// Subscribe using a closure so ctx is captured and available inside the handler.
fn init(ctx) {
    // Read a user-configured greeting prefix.
    // Falls back to "Hello" if the user hasn't changed it yet.
    let prefix = ctx.get_setting("greeting_prefix");
    if prefix == () {
        ctx.set_setting("greeting_prefix", "Hello");
        prefix = "Hello";
    }

    // Subscribe with a closure that captures both ctx and prefix.
    // The closure receives one argument: a map describing the event.
    ctx.subscribe("ProfileChanged", |event| {
        let old_profile = event["old"];
        let new_profile = event["new"];
        let msg = `${prefix}: you switched from "${old_profile}" to "${new_profile}"`;
        ctx.notify("Info", msg);
    });

    ctx.notify("Info", "Profile Greeter loaded.");
}

// shutdown() is called when the plugin is unloaded.
// Subscription handles are dropped automatically — nothing to clean up.
fn shutdown() {}
