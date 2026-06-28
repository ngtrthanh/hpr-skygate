fn main() {
    // Only link librtlsdr if the feature is used (SDR device present)
    // For now, conditionally link — won't fail on systems without it
    if std::env::var("CARGO_FEATURE_SDR").is_ok() {
        println!("cargo:rustc-link-lib=rtlsdr");
    }
}
