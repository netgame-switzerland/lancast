//! Sets the version string (`LANCAST_VERSION`), embeds the icon and version
//! info into LANcast.exe, and copies the NDI runtime DLL next to the built
//! binaries so that LANcast.exe and the examples start without the NDI
//! Runtime being installed.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const NDI_DLL: &str = "Processing.NDI.Lib.x64.dll";
const DEFAULT_SDK_DIR: &str = r"C:\Program Files\NDI\NDI 6 SDK";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let version = version();
    println!("cargo:rustc-env=LANCAST_VERSION={version}");
    embed_exe_resources(&version);
    println!("cargo:rerun-if-env-changed=NDI_SDK_DIR");

    let sdk_dir = env::var_os("NDI_SDK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SDK_DIR));
    let dll = sdk_dir.join("Bin").join("x64").join(NDI_DLL);
    if !dll.is_file() {
        println!(
            "cargo:warning=NDI DLL not found at {}; set NDI_SDK_DIR",
            dll.display()
        );
        return;
    }
    println!("cargo:rerun-if-changed={}", dll.display());

    // OUT_DIR is <target>/<profile>/build/<pkg>-<hash>/out.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        println!(
            "cargo:warning=unexpected OUT_DIR layout: {}",
            out_dir.display()
        );
        return;
    };

    for dir in [profile_dir.to_path_buf(), profile_dir.join("examples")] {
        let target = dir.join(NDI_DLL);
        // Skip identical copies: the DLL may be loaded by a running LANcast,
        // and rewriting an unchanged file would only fail.
        if same_file_contents(&dll, &target) {
            continue;
        }
        if let Err(err) = fs::create_dir_all(&dir).and_then(|()| fs::copy(&dll, &target)) {
            println!(
                "cargo:warning=failed to copy {NDI_DLL} to {}: {err}",
                dir.display()
            );
        }
    }
}

/// The version shown in the UI, the log and the exe's ProductVersion (D30).
/// Release builds (`LANCAST_RELEASE=1`, set by `tools\package.ps1 -Release`
/// in the GitHub release workflow) use the plain Cargo.toml version, e.g.
/// `0.2.0`. Every other build is a snapshot: `0.2.0+snapshot.g1a2b3c4`, with
/// `.dirty` for uncommitted changes. SemVer build metadata (`+…`) marks the
/// build without claiming a newer or older version.
fn version() -> String {
    println!("cargo:rerun-if-env-changed=LANCAST_RELEASE");
    let base = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");
    if env::var("LANCAST_RELEASE").as_deref() == Ok("1") {
        return base;
    }
    let mut version = format!("{base}+snapshot");
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        // Rerun when the commit or the index changes; `src` catches edits
        // that make the tree dirty. Missing files are skipped: cargo would
        // rerun the script on every build for them.
        let git_dir = PathBuf::from(git_dir);
        let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
        let watched = ["HEAD", "index", "packed-refs"]
            .into_iter()
            .chain(head_ref.as_deref())
            .map(|path| git_dir.join(path))
            .filter(|path| path.exists());
        for path in watched {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        println!("cargo:rerun-if-changed=src");
        if let Some(hash) = git(&["rev-parse", "--short=7", "HEAD"]) {
            version += &format!(".g{hash}");
        }
        // --no-optional-locks: don't refresh the index, which would trigger
        // a rerun on every build.
        let status = [
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ];
        if git(&status).is_some_and(|s| !s.is_empty()) {
            version += ".dirty";
        }
    }
    version
}

/// Runs git in the package directory; `None` if git is missing or fails.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Icon and version resource for LANcast.exe (Explorer, taskbar, Alt-Tab).
/// The numeric FileVersion comes from Cargo.toml; ProductVersion is the full
/// version string (also what `tools\package.ps1` reads for the file names).
fn embed_exe_resources(version: &str) {
    println!("cargo:rerun-if-changed=assets/LANcast.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/LANcast.ico")
        .set("ProductName", "LANcast")
        .set(
            "FileDescription",
            "LANcast – send a game window as an NDI source",
        )
        .set("OriginalFilename", "LANcast.exe")
        .set("ProductVersion", version)
        .set(
            "LegalCopyright",
            "Copyright 2026 Kay Huber. Apache License 2.0.",
        );
    if let Err(err) = res.compile() {
        println!("cargo:warning=failed to embed the exe icon: {err}");
    }
}

/// The copy has the same size and is not older than the source.
fn same_file_contents(source: &Path, target: &Path) -> bool {
    let (Ok(src), Ok(dst)) = (fs::metadata(source), fs::metadata(target)) else {
        return false;
    };
    let newer_or_equal = match (src.modified(), dst.modified()) {
        (Ok(s), Ok(d)) => d >= s,
        _ => false,
    };
    src.len() == dst.len() && newer_or_equal
}
