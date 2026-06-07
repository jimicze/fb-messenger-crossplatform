fn main() {
    // Derive a human-readable build version for log identification.
    // `git describe --tags --long --dirty` yields e.g. "v1.5.7-3-gafc7ffe-dirty"
    // (tag + commits-since-tag + short-hash + dirty flag).  Falls back to
    // CARGO_PKG_VERSION when git is unavailable or the repo has no tags yet.
    let build_version = std::process::Command::new("git")
        .args(["describe", "--tags", "--long", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned())
        });

    println!("cargo:rustc-env=MESSENGERX_BUILD_VERSION={build_version}");
    // Rebuild when HEAD moves (commit, checkout) or refs change (tag, branch).
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");

    tauri_build::build()
}
