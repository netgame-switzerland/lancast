//! Windows-only spike; the code is in `windows/wgc_window.rs` so the examples
//! still build on other platforms (D32).

#[cfg(windows)]
#[path = "windows/wgc_window.rs"]
mod spike;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    spike::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This example needs Windows.");
}
