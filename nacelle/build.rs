// Supports io_uring
#[allow(dead_code)]
const MONOIO_RUNTIME: &str = "monoio-runtime";

// Supports IOCP/EPOLL/io_uring
#[allow(dead_code)]
const COMPIO_RUNTIME: &str = "compio-runtime";

/// Nacelle build script.
///
/// Scans every active Cargo feature for names ending in `-runtime` and emits a
/// hard build error if the count is anything other than exactly one.  This
/// works automatically for any future runtime feature — no maintenance needed
/// beyond adding the feature to `Cargo.toml`.
fn main() {
    // Tell Cargo to re-run this script only when Cargo.toml changes (features
    // are encoded there; they don't live in source files).
    println!("cargo::rerun-if-changed=Cargo.toml");

    // Cargo sets CARGO_FEATURE_<NAME> (uppercased, hyphens → underscores) for
    // every active feature.  Collect the original hyphenated names of all
    // *-runtime features by scanning the environment.
    let runtime_features: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| {
            let feat = key.strip_prefix("CARGO_FEATURE_")?;
            // Reconstruct the lowercase-hyphenated feature name.
            let name = feat.to_lowercase().replace('_', "-");
            if name.ends_with("-runtime") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    // If not on Linux, monoio-runtime is not supported
    #[cfg(not(target_os = "linux"))]
    {
        if runtime_features.contains(&MONOIO_RUNTIME.to_string()) {
            panic!(
                "nacelle: monoio-runtime feature is enabled, which is not supported on this platform"
            );
        }
    }

    match runtime_features.len() {
        1 => {} // exactly one — all good
        0 => {
            // none which is fine - no runtime feature is enabled and is in library mode only
            // panic!(
            //     "\n\nnacelle: no runtime feature is enabled.\n\
            //      Enable exactly one feature whose name ends in `-runtime`\n\
            //      (e.g. `tokio-runtime` or `monoio-runtime`).\n"
            // );
        }
        n => {
            let mut names = runtime_features.clone();
            names.sort();
            panic!(
                "\n\nnacelle: {n} runtime features are active simultaneously: {names:?}\n\
                 Enable exactly one feature whose name ends in `-runtime`.\n\
                 Disable the others or remove them from your dependency's `features` list.\n"
            );
        }
    }
}
