//! Embeds the icon and version info into LANcast.exe, and copies the NDI
//! runtime DLL next to the built binaries so that LANcast.exe and the
//! examples start without the NDI Runtime being installed.

use std::{env, fs, path::PathBuf};

const NDI_DLL: &str = "Processing.NDI.Lib.x64.dll";
const DEFAULT_SDK_DIR: &str = r"C:\Program Files\NDI\NDI 6 SDK";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    embed_exe_resources();
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
        if let Err(err) = fs::create_dir_all(&dir).and_then(|()| fs::copy(&dll, dir.join(NDI_DLL)))
        {
            println!(
                "cargo:warning=failed to copy {NDI_DLL} to {}: {err}",
                dir.display()
            );
        }
    }
}

/// Icon and version resource for LANcast.exe (Explorer, taskbar, Alt-Tab).
fn embed_exe_resources() {
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
        .set("OriginalFilename", "LANcast.exe");
    if let Err(err) = res.compile() {
        println!("cargo:warning=failed to embed the exe icon: {err}");
    }
}
