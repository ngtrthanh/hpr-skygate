fn main() {
    println!("cargo:rustc-env=GIT_HASH={}", std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into()));
    if std::env::var("CARGO_FEATURE_SDR").is_ok() {
        println!("cargo:rustc-link-lib=rtlsdr");
    }
}
