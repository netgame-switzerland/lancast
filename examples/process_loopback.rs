//! Windows-only spike; the code is in `windows/process_loopback.rs` so the examples
//! still build on other platforms (D32).

#[cfg(windows)]
#[path = "windows/process_loopback.rs"]
mod spike;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    spike::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This example needs Windows.");
}
