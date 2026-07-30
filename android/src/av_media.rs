//! Native MoQ media plane for freeq AV calls.
//!
//! Publishes mic Opus (+ optional camera H.264 on desktop) to the SFU and
//! plays remote audio/video via `iroh-live` + `moq-native` — same stack as
//! freeq-av / freeq-sdk-ffi.
//!
//! Android: mic/speaker via cpal aaudio; local camera publish is not wired
//! yet (needs CameraX / MediaCodec JNI, not cargo-apk NativeActivity). Remote
//! H.264 still decodes in-software when peers publish video.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh_live::media::{
    audio_backend::AudioBackend,
    codec::AudioCodec,
    format::AudioPreset,
    publish::LocalBroadcast,
    subscribe::RemoteBroadcast,
};
use tokio::sync::oneshot;

use crate::av::{broadcast_path, path_nick, should_tap, VideoFrameStore};

#[cfg(not(target_os = "android"))]
use iroh_live::media::{
    capture::{CameraCapturer, CameraConfig, CameraSelector},
    codec::VideoCodec,
    format::VideoPreset,
    traits::VideoSource,
};

#[derive(Clone)]
pub struct AvMediaConfig {
    pub sfu_url: url::Url,
    pub session_id: String,
    pub nick: String,
    pub instance: String,
    /// Initial mic mute (pre-call preference).
    pub muted: bool,
    /// Initial camera publish when hardware is available.
    pub camera_enabled: bool,
}

/// Status updates from the media task.
#[derive(Debug, Clone)]
pub enum AvMediaUpdate {
    /// MoQ connected and publishing. `has_camera` is true when a capture
    /// device was opened and a video track is advertised.
    Live {
        video: VideoFrameStore,
        has_camera: bool,
    },
    /// Session ended cleanly (stop / transport closed).
    Ended,
    /// Connect or runtime failure.
    Failed(String),
}

/// Background handle: drop or send on `stop` to tear down the MoQ session.
pub struct AvMediaSession {
    stop: Option<oneshot::Sender<()>>,
    pub muted: Arc<AtomicBool>,
    pub camera_enabled: Arc<AtomicBool>,
    pub video: VideoFrameStore,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl AvMediaSession {
    pub fn start<F>(config: AvMediaConfig, on_status: F) -> Self
    where
        F: Fn(AvMediaUpdate) + Send + Sync + 'static,
    {
        let muted = Arc::new(AtomicBool::new(config.muted));
        // Respect pre-call camera pref; has_camera is reported after open.
        let camera_enabled = Arc::new(AtomicBool::new(config.camera_enabled));
        let video = VideoFrameStore::new();
        let (stop_tx, stop_rx) = oneshot::channel();
        let muted_task = muted.clone();
        let camera_task = camera_enabled.clone();
        let video_task = video.clone();
        let on_status = Arc::new(on_status);
        let task = tokio::spawn(async move {
            match run_media(
                config,
                muted_task,
                camera_task,
                video_task,
                stop_rx,
                on_status.clone(),
            )
            .await
            {
                Ok(()) => on_status(AvMediaUpdate::Ended),
                Err(e) => on_status(AvMediaUpdate::Failed(e.to_string())),
            }
        });
        Self {
            stop: Some(stop_tx),
            muted,
            camera_enabled,
            video,
            task: Some(task),
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_camera_enabled(&self, enabled: bool) {
        self.camera_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            // Soft stop first (oneshot); abort if the task is stuck.
            // and moq_lite logs a WARN per stream.
            task.abort();
        }
        self.video.clear();
    }
}

impl Drop for AvMediaSession {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_media(
    config: AvMediaConfig,
    muted: Arc<AtomicBool>,
    camera_enabled: Arc<AtomicBool>,
    video_store: VideoFrameStore,
    mut stop: oneshot::Receiver<()>,
    on_status: Arc<dyn Fn(AvMediaUpdate) + Send + Sync>,
) -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let our_broadcast = broadcast_path(&config.session_id, &config.nick, &config.instance);
    log::info!(
        "av-media: dialing {} as {our_broadcast}",
        config.sfu_url
    );

    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init().context("moq client init")?;

    let broadcast = LocalBroadcast::new();
    let audio_backend = AudioBackend::default();
    audio_backend.set_aec_enabled(false);

    let mic = audio_backend
        .default_input()
        .await
        .context("open default microphone")?;
    let muteable = MuteableSource {
        inner: Box::new(mic),
        muted: muted.clone(),
    };
    broadcast
        .audio()
        .set(muteable, AudioCodec::Opus, [AudioPreset::Hq])
        .context("set mic audio source")?;

    // Camera: desktop only (V4L2 / platform capture). Android NativeActivity
    // has no CameraX bridge yet — audio-only publish; remote video still works.
    #[cfg(not(target_os = "android"))]
    let has_camera = {
        match open_camera() {
            Ok(cam) => {
                let gated = GatedCameraSource {
                    inner: cam,
                    enabled: camera_enabled.clone(),
                    preview: video_store.clone(),
                };
                match broadcast
                    .video()
                    .set_source(gated, VideoCodec::H264, [VideoPreset::P360])
                {
                    Ok(()) => {
                        log::info!("av-media: camera open, publishing H.264 360p");
                        true
                    }
                    Err(e) => {
                        log::warn!("av-media: video set_source failed (audio-only): {e}");
                        camera_enabled.store(false, Ordering::Relaxed);
                        false
                    }
                }
            }
            Err(e) => {
                log::warn!("av-media: no camera ({e}); audio-only");
                camera_enabled.store(false, Ordering::Relaxed);
                false
            }
        }
    };
    #[cfg(target_os = "android")]
    let has_camera = {
        camera_enabled.store(false, Ordering::Relaxed);
        log::info!("av-media: Android — audio-only publish (no local camera yet)");
        false
    };

    let origin = moq_lite::Origin::produce();
    origin.publish_broadcast(&our_broadcast, broadcast.consume());

    let sub_origin = moq_lite::Origin::produce();
    let mut sub_consumer = sub_origin.consume();

    let session_handle = client
        .with_publish(origin.consume())
        .with_consume(sub_origin)
        .connect(config.sfu_url.clone())
        .await
        .context("MoQ connect")?;

    log::info!("av-media: MoQ connected, publishing {our_broadcast}");
    on_status(AvMediaUpdate::Live {
        video: video_store.clone(),
        has_camera,
    });

    let audio_for_playback = audio_backend.clone();
    let session_id = config.session_id.clone();
    let my_nick = config.nick.clone();
    let our_name = our_broadcast.clone();
    let store_for_subs = video_store.clone();
    let sub_task = tokio::spawn(async move {
        while let Some((path, announce)) = sub_consumer.announced().await {
            match announce {
                Some(broadcast_consumer) => {
                    let path_str = path.to_string();
                    if !should_tap(&path_str, &session_id, &our_name, &my_nick) {
                        continue;
                    }
                    log::info!("av-media: + remote {path_str}");
                    let ab = audio_for_playback.clone();
                    let ps = path_str.clone();
                    let store = store_for_subs.clone();
                    let nick = path_nick(&path_str).to_string();
                    tokio::spawn(async move {
                        tap_remote(ps, nick, broadcast_consumer, ab, store).await;
                    });
                }
                None => {
                    let path_str = path.to_string();
                    let nick = path_nick(&path_str).to_string();
                    store_for_subs.remove(&nick);
                    log::info!("av-media: - remote {path_str}");
                }
            }
        }
    });

    let _broadcast = broadcast;
    tokio::select! {
        res = session_handle.closed() => {
            if let Err(e) = res {
                // "connection closed" is expected on leave / peer hangup / SFU
                // idle timeout — one line is enough; moq_lite may still WARN
                // per stream internally.
                log::info!("av-media: session closed: {e}");
            } else {
                log::info!("av-media: session closed cleanly");
            }
        }
        _ = &mut stop => {
            log::info!("av-media: stop requested");
        }
    }
    // Drop session first so the transport shuts down, then cancel subscribers.
    drop(session_handle);
    sub_task.abort();
    video_store.clear();
    // Brief yield so abort of child tasks and QUIC close settle without
    // racing a hard parent abort (Drop) on the next AvMediaConnect.
    tokio::task::yield_now().await;
    Ok(())
}

/// Subscribe audio + video for one remote broadcast until it ends or is aborted.
async fn tap_remote(
    path: String,
    nick: String,
    broadcast_consumer: moq_lite::BroadcastConsumer,
    audio_backend: AudioBackend,
    video_store: VideoFrameStore,
) {
    let remote = match RemoteBroadcast::new(&path, broadcast_consumer).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("av-media: catalog {path}: {e}");
            return;
        }
    };

    // Audio: keep track alive for playback (same as previous audio-only path).
    let audio_task = {
        let remote = remote.clone();
        let ab = audio_backend;
        let ps = path.clone();
        tokio::spawn(async move {
            match remote.audio(&ab).await {
                Ok(track) => {
                    log::info!("av-media: receiving audio from {ps}");
                    let _track = track;
                    std::future::pending::<()>().await;
                }
                Err(e) => log::warn!("av-media: audio sub {ps}: {e}"),
            }
        })
    };

    // Video: pump latest frames into the shared store (loops on camera toggle).
    let video_task = {
        let remote = remote.clone();
        let store = video_store.clone();
        let nick = nick.clone();
        let path = path.clone();
        tokio::spawn(async move {
            loop {
                match remote.video_ready().await {
                    Ok(mut vtrack) => {
                        log::info!("av-media: receiving video from {path}");
                        while let Some(frame) = vtrack.next_frame().await {
                            let (w, h) = (frame.width(), frame.height());
                            let rgba = frame.rgba_image();
                            let bytes: Arc<[u8]> = rgba.as_raw().as_slice().into();
                            store.set(nick.clone(), w, h, bytes);
                        }
                        log::info!("av-media: video track ended for {path}");
                    }
                    Err(e) => {
                        log::debug!("av-media: video wait {path}: {e}");
                        // Catalog may not advertise video yet; retry briefly.
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        })
    };

    // Stay until either side finishes (or the parent aborts us).
    tokio::select! {
        _ = audio_task => {}
        _ = video_task => {}
    }
    video_store.remove(&nick);
}

#[cfg(not(target_os = "android"))]
fn open_camera() -> Result<CameraCapturer> {
    let config = CameraConfig {
        selector: CameraSelector::TargetResolution(640, 360),
        preferred_format: None,
        zero_copy: true,
    };
    CameraCapturer::with_config(None, &config).context("open camera")
}

/// Wraps camera capture: when disabled, drain frames but publish none so the
/// H.264 track stays in the catalog (peers who already subscribed keep the
/// video rendition for when the camera comes back).
#[cfg(not(target_os = "android"))]
struct GatedCameraSource {
    inner: CameraCapturer,
    enabled: Arc<AtomicBool>,
    preview: VideoFrameStore,
}

#[cfg(not(target_os = "android"))]
impl VideoSource for GatedCameraSource {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn format(&self) -> iroh_live::media::format::VideoFormat {
        self.inner.format()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.inner.start()
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.inner.stop()
    }

    fn pop_frame(
        &mut self,
    ) -> anyhow::Result<Option<iroh_live::media::format::VideoFrame>> {
        let frame = self.inner.pop_frame()?;
        if !self.enabled.load(Ordering::Relaxed) {
            // Drain so the capture pipeline doesn't back up.
            return Ok(None);
        }
        if let Some(ref f) = frame {
            let (w, h) = (f.width(), f.height());
            let rgba = f.rgba_image();
            let bytes: Arc<[u8]> = rgba.as_raw().as_slice().into();
            self.preview.set("__local__", w, h, bytes);
        }
        Ok(frame)
    }
}

/// Wraps an AudioSource and emits silence while muted (keeps the Opus track live).
struct MuteableSource {
    inner: Box<dyn iroh_live::media::traits::AudioSource>,
    muted: Arc<AtomicBool>,
}

impl iroh_live::media::traits::AudioSource for MuteableSource {
    fn format(&self) -> iroh_live::media::format::AudioFormat {
        self.inner.format()
    }

    fn pop_samples(
        &mut self,
        buf: &mut [f32],
    ) -> anyhow::Result<Option<usize>> {
        let result = self.inner.pop_samples(buf)?;
        if let Some(n) = result {
            if self.muted.load(Ordering::Relaxed) {
                for s in &mut buf[..n] {
                    *s = 0.0;
                }
            }
        }
        Ok(result)
    }
}
