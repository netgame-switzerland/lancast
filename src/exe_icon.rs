//! The icon of an executable file as RGBA, for the window carousel (D19).
//! Read from the file via the shell, so nothing is sent to the running
//! process.

use windows::{
    Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
            ReleaseDC,
        },
        Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
        UI::{
            Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW},
            WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO},
        },
    },
    core::HSTRING,
};

/// The large (usually 32×32) icon of the file at `path`: `(width, height, rgba)`.
/// Must be called on a thread with COM initialized (the UI thread is).
pub fn exe_icon_rgba(path: &str) -> Option<(usize, usize, Vec<u8>)> {
    let mut info = SHFILEINFOW::default();
    // SAFETY: `info` is a valid out-structure of the size passed.
    let ok = unsafe {
        SHGetFileInfoW(
            &HSTRING::from(path),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok == 0 || info.hIcon.is_invalid() {
        return None;
    }

    // SAFETY: the icon and the bitmaps from GetIconInfo are released below;
    // GetDIBits writes at most width × height × 4 bytes into `pixels`.
    unsafe {
        let mut icon = ICONINFO::default();
        let result = GetIconInfo(info.hIcon, &mut icon).ok().and_then(|()| {
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = GetDC(None);
            // First call fills in the size.
            let sized = GetDIBits(dc, icon.hbmColor, 0, 0, None, &mut bmi, DIB_RGB_COLORS) != 0;
            let (w, h) = (bmi.bmiHeader.biWidth, bmi.bmiHeader.biHeight.abs());
            let mut pixels = vec![0u8; (w.max(0) * h.max(0) * 4) as usize];
            let read = sized && w > 0 && h > 0 && {
                bmi.bmiHeader.biBitCount = 32;
                bmi.bmiHeader.biCompression = BI_RGB.0;
                bmi.bmiHeader.biHeight = -h; // top-down
                GetDIBits(
                    dc,
                    icon.hbmColor,
                    0,
                    h as u32,
                    Some(pixels.as_mut_ptr().cast()),
                    &mut bmi,
                    DIB_RGB_COLORS,
                ) != 0
            };
            ReleaseDC(None, dc);
            read.then_some((w as usize, h as usize, pixels))
        });
        if !icon.hbmColor.is_invalid() {
            let _ = DeleteObject(icon.hbmColor.into());
        }
        if !icon.hbmMask.is_invalid() {
            let _ = DeleteObject(icon.hbmMask.into());
        }
        let _ = DestroyIcon(info.hIcon);

        let (w, h, mut bgra) = result?;
        // Old icons without alpha: treat as opaque.
        let has_alpha = bgra.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            if !has_alpha {
                px[3] = 255;
            }
        }
        Some((w, h, bgra))
    }
}
