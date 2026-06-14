fn main() {
    let ts = std::process::Command::new("date")
        .args(["+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=BUILD_TIMESTAMP={ts}");
    println!("cargo:rerun-if-changed=build.rs");
}
