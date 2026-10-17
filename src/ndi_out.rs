//! NDI sender thread (SPEC §5.3). It owns the `grafton_ndi::Sender`, takes
//! video frames from the capture thread and returns released frames for reuse.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender as ChannelSender};
use grafton_ndi::{NDI, Sender, SenderOptions, VideoFrame};
use tracing::{debug, info, warn};

/// How often the receiver count is refreshed.
const CONNECTION_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct NdiStats {
    pub video_sent: AtomicU64,
    pub connections: AtomicU32,
}

/// The running sender thread. Stops when dropped.
pub struct NdiOutput {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    /// Full network name, e.g. `MACHINE (war3)`.
    pub source_name: String,
    pub stats: Arc<NdiStats>,
}

impl NdiOutput {
    /// Creates the NDI source `name` and starts sending frames from `frames`.
    /// Frames released by the SDK go back through `recycle`.
    pub fn start(
        name: &str,
        frames: Receiver<VideoFrame>,
        recycle: ChannelSender<VideoFrame>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(NdiStats::default());
        // The sender is created on its own thread; it reports success or the
        // creation error back before streaming starts.
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<String>>(1);

        let thread = std::thread::Builder::new()
            .name("ndi-send".into())
            .spawn({
                let name = name.to_owned();
                let stop = stop.clone();
                let stats = stats.clone();
                move || match create_sender(&name) {
                    Ok((sender, source)) => {
                        let _ = ready_tx.send(Ok(source));
                        run(sender, &frames, &recycle, &stop, &stats);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            })
            .context("failed to spawn NDI thread")?;

        let source_name = ready_rx
            .recv()
            .map_err(|_| anyhow!("NDI thread exited during start-up"))??;
        info!(source = %source_name, "NDI source created");
        Ok(Self {
            thread: Some(thread),
            stop,
            source_name,
            stats,
        })
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            warn!("NDI thread panicked");
        }
    }
}

impl Drop for NdiOutput {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// The sender keeps the NDI runtime alive for its own lifetime.
fn create_sender(name: &str) -> Result<(Sender, String)> {
    let ndi = NDI::new().context("failed to initialize the NDI runtime")?;
    // Video clocks the stream; audio (M2) is submitted from this same thread
    // and must not be clocked as well (D3).
    let options = SenderOptions::builder(name)
        .clock_video(true)
        .clock_audio(false)
        .build();
    let sender = Sender::new(&ndi, &options).context("failed to create the NDI sender")?;
    let source = sender
        .source()
        .map(|s| s.to_string())
        .unwrap_or_else(|_| name.to_owned());
    Ok((sender, source))
}

fn run(
    mut sender: Sender,
    frames: &Receiver<VideoFrame>,
    recycle: &ChannelSender<VideoFrame>,
    stop: &AtomicBool,
    stats: &NdiStats,
) {
    let mut last_poll = Instant::now() - CONNECTION_POLL;
    while !stop.load(Ordering::Relaxed) {
        match frames.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                // With clock_video this blocks until the frame's slot.
                if let Some(released) = sender.send_video_async(frame) {
                    // A full pool just means the capture side has enough.
                    let _ = recycle.try_send(released);
                }
                stats.video_sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                debug!("video channel closed");
                break;
            }
        }

        if last_poll.elapsed() >= CONNECTION_POLL {
            last_poll = Instant::now();
            match sender.connection_count(Duration::ZERO) {
                Ok(count) => {
                    let previous = stats.connections.swap(count, Ordering::Relaxed);
                    if previous != count {
                        info!(receivers = count, "NDI receivers changed");
                    }
                }
                Err(err) => warn!(%err, "failed to query NDI connections"),
            }
        }
    }
    // Wait until the SDK has released the last async frame before dropping it.
    sender.finish_video_async();
}
