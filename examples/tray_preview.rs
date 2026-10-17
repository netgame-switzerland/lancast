//! Renders the tray icon in all states at the tray sizes (16–32 px) on a
//! light and a dark background, 4× enlarged, into
//! `docs/images/tray-states-4x.png`.
//!
//! Usage: `cargo run --release --example tray_preview`

use std::{fs::File, io::BufWriter};

use lancast::tray::{TrayState, tray_icon_rgba};

const SIZES: [usize; 4] = [16, 20, 24, 32];
const STATES: [TrayState; 4] = [
    TrayState::Idle,
    TrayState::Sending,
    TrayState::SendingSystemAudio,
    TrayState::Error,
];
const CELL: usize = 44;
const ZOOM: usize = 4;

fn main() -> anyhow::Result<()> {
    let width = CELL * SIZES.len() * 2;
    let height = CELL * STATES.len();
    let mut sheet = vec![0u8; width * height * 4];
    for (row, state) in STATES.into_iter().enumerate() {
        for (half, background) in [[243u8, 243, 243], [32, 32, 32]].into_iter().enumerate() {
            for (col, size) in SIZES.into_iter().enumerate() {
                let x0 = (half * SIZES.len() + col) * CELL;
                let y0 = row * CELL;
                fill(&mut sheet, width, x0, y0, CELL, background);
                let (side, icon) = tray_icon_rgba(size, state);
                let offset = (CELL - side) / 2;
                blend(&mut sheet, width, x0 + offset, y0 + offset, side, &icon);
            }
        }
    }
    let zoomed = zoom(&sheet, width, height);
    let path = "docs/images/tray-states-4x.png";
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        (width * ZOOM) as u32,
        (height * ZOOM) as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header()?.write_image_data(&zoomed)?;
    println!("wrote {path} (rows: idle, sending, system audio, error; light | dark)");
    Ok(())
}

fn fill(sheet: &mut [u8], width: usize, x0: usize, y0: usize, n: usize, rgb: [u8; 3]) {
    for y in y0..y0 + n {
        for x in x0..x0 + n {
            sheet[(y * width + x) * 4..][..4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
}

fn blend(sheet: &mut [u8], width: usize, x0: usize, y0: usize, side: usize, icon: &[u8]) {
    for y in 0..side {
        for x in 0..side {
            let src = &icon[(y * side + x) * 4..][..4];
            let a = u32::from(src[3]);
            let dst = &mut sheet[((y0 + y) * width + x0 + x) * 4..][..3];
            for c in 0..3 {
                dst[c] = ((u32::from(src[c]) * a + u32::from(dst[c]) * (255 - a)) / 255) as u8;
            }
        }
    }
}

fn zoom(sheet: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4 * ZOOM * ZOOM];
    for y in 0..height * ZOOM {
        for x in 0..width * ZOOM {
            let src = &sheet[((y / ZOOM) * width + x / ZOOM) * 4..][..4];
            out[(y * width * ZOOM + x) * 4..][..4].copy_from_slice(src);
        }
    }
    out
}
