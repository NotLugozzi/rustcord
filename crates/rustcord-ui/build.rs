use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");

    emit("RUSTCORD_COMMIT", git(&["rev-parse", "--short", "HEAD"]));
    emit("RUSTCORD_TAG", git(&["describe", "--tags", "--exact-match", "HEAD"]));
    emit("RUSTCORD_COMMIT_DATE", git(&["log", "-1", "--format=%cs"]));

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/swift");
        let swiftc = Command::new("xcrun").args(["--find", "swiftc"]).output().ok();
        let dir = swiftc
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|path| std::path::Path::new(path.trim()).parent()?.parent().map(|usr| usr.join("lib/swift/macosx")));
        if let Some(dir) = dir.filter(|dir| dir.exists()) {
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn emit(key: &str, value: Option<String>) {
    println!("cargo:rustc-env={key}={}", value.unwrap_or_default());
}
