/// Emit `RUSTC_VERSION_STRING` at compile time so that
/// `mantle_core::plugin::RUSTC_TOOLCHAIN_VERSION` can be populated via
/// `env!("RUSTC_VERSION_STRING")`.
///
/// This string is compared against the same constant exported by native
/// plugin `.so` files (`create_plugin_rustc_version()`) to enforce ABI
/// compatibility at load time.  See `PLUGIN_API.md` §3.2.
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    let version = String::from_utf8(output.stdout).expect("rustc --version output is valid UTF-8");
    // Emit the version string (trimmed) as a compile-time env var.
    println!("cargo:rustc-env=RUSTC_VERSION_STRING={}", version.trim());
    // Re-run only if the toolchain changes (rare).
    println!("cargo:rerun-if-env-changed=RUSTC");
}
