//! Live window thumbnails via DWM (`DwmRegisterThumbnail`), the mechanism
//! Alt-Tab uses (D19). The compositor draws a scaled live copy of a source
//! window into our window; nothing is sent to the source process, and it
//! costs us no CPU.

use std::collections::HashMap;

use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Dwm::{
        DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
        DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize,
        DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
    },
};

/// Thumbnails of several source windows inside one destination window.
pub struct Thumbnails {
    destination: isize,
    handles: HashMap<isize, isize>,
}

impl Thumbnails {
    pub fn new(destination: isize) -> Self {
        Self {
            destination,
            handles: HashMap::new(),
        }
    }

    /// Size of the source window in pixels, registering it if needed.
    pub fn source_size(&mut self, source: isize) -> Option<(i32, i32)> {
        let handle = self.handle(source)?;
        // SAFETY: `handle` is a registered thumbnail.
        let size = unsafe { DwmQueryThumbnailSourceSize(handle) }.ok()?;
        (size.cx > 0 && size.cy > 0).then_some((size.cx, size.cy))
    }

    /// Shows `source` in `dest` (client pixels of the destination window),
    /// cropped to `clip`, keeping the source aspect ratio within `dest`.
    pub fn show(&mut self, source: isize, dest: [i32; 4], clip: [i32; 4]) {
        let Some((sw, sh)) = self.source_size(source) else {
            return;
        };
        let Some(handle) = self.handle(source) else {
            return;
        };
        // Fit the source into dest, centered.
        let (dw, dh) = ((dest[2] - dest[0]) as f32, (dest[3] - dest[1]) as f32);
        let scale = (dw / sw as f32).min(dh / sh as f32);
        let (w, h) = (sw as f32 * scale, sh as f32 * scale);
        let x0 = dest[0] as f32 + (dw - w) / 2.0;
        let y0 = dest[1] as f32 + (dh - h) / 2.0;
        let full = [x0, y0, x0 + w, y0 + h];
        // Clip to the visible area and crop the source accordingly.
        let shown = [
            full[0].max(clip[0] as f32),
            full[1].max(clip[1] as f32),
            full[2].min(clip[2] as f32),
            full[3].min(clip[3] as f32),
        ];
        let visible = shown[2] - shown[0] >= 2.0 && shown[3] - shown[1] >= 2.0;
        let to_source = |v: f32, origin: f32| ((v - origin) / scale).round() as i32;
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION
                | DWM_TNP_RECTSOURCE
                | DWM_TNP_VISIBLE
                | DWM_TNP_OPACITY
                | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: RECT {
                left: shown[0].round() as i32,
                top: shown[1].round() as i32,
                right: shown[2].round() as i32,
                bottom: shown[3].round() as i32,
            },
            rcSource: RECT {
                left: to_source(shown[0], full[0]),
                top: to_source(shown[1], full[1]),
                right: to_source(shown[2], full[0]),
                bottom: to_source(shown[3], full[1]),
            },
            opacity: 255,
            fVisible: visible.into(),
            fSourceClientAreaOnly: false.into(),
        };
        tracing::trace!(
            source,
            ?dest,
            ?clip,
            src = ?(sw, sh),
            shown = ?shown,
            "thumbnail"
        );
        // SAFETY: `handle` is registered and `props` is fully initialized.
        let _ = unsafe { DwmUpdateThumbnailProperties(handle, &props) };
    }

    /// Hides `source` without unregistering it.
    pub fn hide(&mut self, source: isize) {
        if let Some(&handle) = self.handles.get(&source) {
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_VISIBLE,
                fVisible: false.into(),
                ..Default::default()
            };
            // SAFETY: as above.
            let _ = unsafe { DwmUpdateThumbnailProperties(handle, &props) };
        }
    }

    /// Unregisters thumbnails whose source is not in `keep`.
    pub fn retain(&mut self, keep: impl Fn(isize) -> bool) {
        self.handles.retain(|&source, &mut handle| {
            let keep = keep(source);
            if !keep {
                // SAFETY: unregistering a handle we registered.
                let _ = unsafe { DwmUnregisterThumbnail(handle) };
            }
            keep
        });
    }

    pub fn clear(&mut self) {
        self.retain(|_| false);
    }

    fn handle(&mut self, source: isize) -> Option<isize> {
        if let Some(&handle) = self.handles.get(&source) {
            return Some(handle);
        }
        // SAFETY: both are window handles; registration fails cleanly for
        // stale ones.
        let handle = unsafe {
            DwmRegisterThumbnail(HWND(self.destination as *mut _), HWND(source as *mut _))
        }
        .ok()?;
        self.handles.insert(source, handle);
        Some(handle)
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        self.clear();
    }
}
