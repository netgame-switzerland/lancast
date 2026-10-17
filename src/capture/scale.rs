//! CPU copy and bilinear downscaling of 32-bit BGRA/BGRX frames (SPEC §5.1).
//! v2 moves this to the GPU (SPEC §5.5).

use rayon::prelude::*;

/// Largest size with the source aspect ratio that fits `max`, never
/// upscaling. The width is even, as NDI's 4:2:2 encoding requires.
pub fn fit_size(src: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let (w, h) = (src.0.max(1), src.1.max(1));
    let scale = f64::min(max.0 as f64 / w as f64, max.1 as f64 / h as f64).min(1.0);
    let out_w = ((w as f64 * scale).round() as u32).max(2) & !1;
    let out_h = ((h as f64 * scale).round() as u32).max(1);
    (out_w, out_h)
}

/// Copies or scales `src` (rows of `src_pitch` bytes) into the tightly packed
/// `dst` of `dst_size`. Equal widths and heights copy rows; anything else is
/// bilinearly resampled.
pub fn copy_or_scale(
    src: &[u8],
    src_size: (u32, u32),
    src_pitch: usize,
    dst: &mut [u8],
    dst_size: (u32, u32),
) {
    let (src_w, src_h) = (src_size.0 as usize, src_size.1 as usize);
    let (dst_w, dst_h) = (dst_size.0 as usize, dst_size.1 as usize);
    debug_assert!(src.len() >= src_pitch * (src_h - 1) + src_w * 4);
    debug_assert_eq!(dst.len(), dst_w * dst_h * 4);

    if dst_w == src_w && dst_h == src_h {
        dst.par_chunks_exact_mut(dst_w * 4)
            .enumerate()
            .for_each(|(y, row)| row.copy_from_slice(&src[y * src_pitch..][..dst_w * 4]));
        return;
    }

    // Per destination column: byte offsets of the two source pixels and the
    // 8-bit weight of the right one. Pixel centers are aligned.
    let xs: Vec<(usize, usize, u32)> = (0..dst_w)
        .map(|x| {
            let (i0, i1, w) = taps(x, dst_w, src_w);
            (i0 * 4, i1 * 4, w)
        })
        .collect();

    dst.par_chunks_exact_mut(dst_w * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let (y0, y1, wy) = taps(y, dst_h, src_h);
            let top = &src[y0 * src_pitch..][..src_w * 4];
            let bottom = &src[y1 * src_pitch..][..src_w * 4];
            for (px, &(a, b, wx)) in row.as_chunks_mut::<4>().0.iter_mut().zip(&xs) {
                for c in 0..3 {
                    let t = u32::from(top[a + c]) * (256 - wx) + u32::from(top[b + c]) * wx;
                    let m = u32::from(bottom[a + c]) * (256 - wx) + u32::from(bottom[b + c]) * wx;
                    px[c] = ((t * (256 - wy) + m * wy + (1 << 15)) >> 16) as u8;
                }
                px[3] = 255;
            }
        });
}

/// Downscale of a BGRX frame (rows of `src_pitch` bytes) to RGBA for the UI
/// preview by area averaging (each output pixel is the mean of the source
/// pixels it covers), fitting `max` without upscaling. Returns the size and
/// the pixels.
pub fn preview_rgba(
    src: &[u8],
    src_size: (u32, u32),
    src_pitch: usize,
    max: (u32, u32),
) -> ((u32, u32), Vec<u8>) {
    let (w, h) = fit_size(src_size, max);
    let (src_w, src_h) = (src_size.0 as usize, src_size.1 as usize);
    let (w, h) = (w as usize, h as usize);
    // Source column range [start, end) per output column.
    let columns: Vec<(usize, usize)> = (0..w)
        .map(|x| {
            let start = x * src_w / w;
            let end = ((x + 1) * src_w / w).clamp(start + 1, src_w);
            (start, end)
        })
        .collect();
    let mut rgba = vec![0u8; w * h * 4];
    rgba.par_chunks_exact_mut(w * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let y0 = y * src_h / h;
            let y1 = ((y + 1) * src_h / h).clamp(y0 + 1, src_h);
            for (px, &(x0, x1)) in row.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
                let mut sum = [0u32; 3];
                for sy in y0..y1 {
                    let line = &src[sy * src_pitch + x0 * 4..sy * src_pitch + x1 * 4];
                    for p in line.as_chunks::<4>().0 {
                        sum[0] += u32::from(p[2]);
                        sum[1] += u32::from(p[1]);
                        sum[2] += u32::from(p[0]);
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                *px = [
                    ((sum[0] + n / 2) / n) as u8,
                    ((sum[1] + n / 2) / n) as u8,
                    ((sum[2] + n / 2) / n) as u8,
                    255,
                ];
            }
        });
    ((w as u32, h as u32), rgba)
}

/// Source indices and 8-bit weight of the second index for destination
/// index `i`.
fn taps(i: usize, dst_len: usize, src_len: usize) -> (usize, usize, u32) {
    let pos = ((i as f64 + 0.5) * src_len as f64 / dst_len as f64 - 0.5).max(0.0);
    let i0 = (pos as usize).min(src_len - 1);
    let i1 = (i0 + 1).min(src_len - 1);
    let w = ((pos - i0 as f64) * 256.0).round().min(256.0) as u32;
    (i0, i1, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_size((2560, 1440), (1920, 1080)), (1920, 1080));
        assert_eq!(fit_size((3440, 1440), (1920, 1080)), (1920, 804));
        assert_eq!(fit_size((1280, 720), (1920, 1080)), (1280, 720));
        assert_eq!(fit_size((1281, 721), (1920, 1080)), (1280, 721));
    }

    #[test]
    fn copy_strips_row_padding() {
        let src = [1, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
        let mut dst = [0u8; 8];
        copy_or_scale(&src, (1, 2), 6, &mut dst, (1, 2));
        assert_eq!(dst, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn preview_swizzles_to_rgba() {
        let src: Vec<u8> = [10, 20, 30, 0].repeat(640 * 360);
        let ((w, h), rgba) = preview_rgba(&src, (640, 360), 640 * 4, (320, 180));
        assert_eq!((w, h), (320, 180));
        assert!(rgba.chunks(4).all(|p| p == [30, 20, 10, 255]));
    }

    #[test]
    fn preview_averages_areas() {
        // 4x2 source: left half black, right half white -> 2x1 preview.
        let mut src = Vec::new();
        for _ in 0..2 {
            src.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 0, 255, 255, 255, 0]);
        }
        let ((w, h), rgba) = preview_rgba(&src, (4, 2), 16, (2, 1));
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, [0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn scaling_a_uniform_image_keeps_its_color() {
        let src: Vec<u8> = [10, 20, 30, 255].repeat(64 * 36);
        let mut dst = vec![0u8; 48 * 27 * 4];
        copy_or_scale(&src, (64, 36), 64 * 4, &mut dst, (48, 27));
        assert!(dst.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }
}
