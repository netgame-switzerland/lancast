//! The LANcast icon (assets/icon-source.png, scaled by tools/make_icons.py)
//! as RGBA for the window and the tray. The exe icon is embedded separately
//! as a Windows resource (build.rs).

use std::io::Cursor;

/// Pre-scaled PNGs, so small sizes look sharp at every DPI.
const ICONS: [(usize, &[u8]); 7] = [
    (16, include_bytes!("../assets/icon-16.png")),
    (20, include_bytes!("../assets/icon-20.png")),
    (24, include_bytes!("../assets/icon-24.png")),
    (32, include_bytes!("../assets/icon-32.png")),
    (40, include_bytes!("../assets/icon-40.png")),
    (48, include_bytes!("../assets/icon-48.png")),
    (64, include_bytes!("../assets/icon-64.png")),
];

/// The icon for a requested size: the smallest embedded size that is at
/// least `size` (or the largest). Returns `(side, rgba)`.
pub fn icon_rgba(size: usize) -> (usize, Vec<u8>) {
    let (side, bytes) = ICONS
        .iter()
        .find(|(side, _)| *side >= size)
        .unwrap_or(&ICONS[ICONS.len() - 1]);
    let rgba = decode(bytes).unwrap_or_else(|| vec![0; side * side * 4]);
    (*side, rgba)
}

fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());
    match info.color_type {
        png::ColorType::Rgba => Some(buffer),
        png::ColorType::Rgb => Some(
            buffer
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|[r, g, b]| [*r, *g, *b, 255])
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icons_decode_at_their_size() {
        for (side, _) in ICONS {
            let (got, rgba) = icon_rgba(side);
            assert_eq!(got, side);
            assert_eq!(rgba.len(), side * side * 4);
            assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        }
        assert_eq!(icon_rgba(18).0, 20);
        assert_eq!(icon_rgba(500).0, 64);
    }
}
