//! M0 spike: print capturable windows with HWND, PID and process name.
//!
//! Usage: `cargo run --release --example list_windows`

fn main() -> anyhow::Result<()> {
    let windows = lancast::windows_list::list_windows()?;
    println!(
        "{:<12} {:>7} {:<28} {:>11}  {:<24} TITLE",
        "HWND", "PID", "PROCESS", "SIZE", "CLASS"
    );
    for w in &windows {
        let size = if w.minimized {
            "minimized".to_owned()
        } else {
            format!("{}x{}", w.width, w.height)
        };
        println!(
            "{:<12} {:>7} {:<28} {:>11}  {:<24} {}",
            format!("0x{:x}", w.hwnd),
            w.pid,
            w.process_name.as_deref().unwrap_or("<access denied>"),
            size,
            w.class_name,
            w.title
        );
    }
    println!("{} windows", windows.len());
    Ok(())
}
