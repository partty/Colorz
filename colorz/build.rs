//! Compiles `data/` resources (the app icon) into the binary, and on
//! Windows also links `windows/colorz.rc` (exe icon and version info).

fn main() {
    glib_build_tools::compile_resources(
        &["../data"],
        "../data/io.github.partty.Colorz.gresource.xml",
        "colorz.gresource",
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

/// Compiles `windows/colorz.rc` with `$WINDRES` (default `windres`) and
/// links the object into the binaries. `COLORZ_SKIP_RC` skips this, for
/// type-checking without a Windows resource compiler.
fn windows_resources() {
    println!("cargo:rerun-if-changed=../windows/colorz.rc");
    println!("cargo:rerun-if-changed=../windows/colorz.ico");
    println!("cargo:rerun-if-env-changed=COLORZ_SKIP_RC");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if std::env::var("COLORZ_SKIP_RC").is_ok() {
        return;
    }
    let env = |key: &str| std::env::var(key).unwrap_or_else(|e| panic!("{key}: {e}"));
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "windres".to_owned());
    let version = env("CARGO_PKG_VERSION");
    let num = format!(
        "{},{},{},0",
        env("CARGO_PKG_VERSION_MAJOR"),
        env("CARGO_PKG_VERSION_MINOR"),
        env("CARGO_PKG_VERSION_PATCH"),
    );
    let out = std::path::Path::new(&env("OUT_DIR")).join("colorz-rc.o");
    let status = std::process::Command::new(&windres)
        .arg(format!("-DCOLORZ_VERSION_STR=\"{version}\""))
        .arg(format!("-DCOLORZ_VERSION_NUM={num}"))
        .args(["--include-dir", "../windows", "-O", "coff", "-o"])
        .arg(&out)
        .arg("../windows/colorz.rc")
        .status()
        .unwrap_or_else(|e| panic!("cannot run {windres} for windows/colorz.rc: {e}"));
    if !status.success() {
        panic!("{windres} failed on windows/colorz.rc ({status})");
    }
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
