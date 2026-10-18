//! Stub of `win_thumbnail` for platforms without live thumbnails yet (D32):
//! no source has a size, so the carousel falls back to icons.

pub struct Thumbnails;

impl Thumbnails {
    pub fn new(_destination: isize) -> Self {
        Self
    }

    pub fn source_size(&mut self, _source: isize) -> Option<(i32, i32)> {
        None
    }

    pub fn show(&mut self, _source: isize, _dest: [i32; 4], _clip: [i32; 4], _opacity: u8) {}

    pub fn hide(&mut self, _source: isize) {}

    pub fn retain(&mut self, _keep: impl Fn(isize) -> bool) {}

    pub fn clear(&mut self) {}
}
