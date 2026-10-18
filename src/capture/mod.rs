//! Video and audio capture. The portable parts live in `audio` and `video`;
//! the Windows backends in `audio_wasapi` and `video_wgc` (D32).

pub mod audio;
#[cfg(windows)]
mod audio_wasapi;
pub mod pacer;
pub mod scale;
pub mod video;
#[cfg(windows)]
mod video_wgc;
