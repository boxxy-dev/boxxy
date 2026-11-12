use std::process::Command;

fn main() {
    let output = std::env::var("GIT_HASH").unwrap_or_else(|_| {
        Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });

    println!("cargo:rustc-env=GIT_HASH={output}");
    println!("cargo:rerun-if-env-changed=GIT_HASH");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(ref_path) = std::fs::read_to_string("../../.git/HEAD")
        && let Some(stripped) = ref_path.strip_prefix("ref: ")
    {
        let path = format!("../../.git/{}", stripped.trim());
        println!("cargo:rerun-if-changed={path}");
    }
}
