//! Native MoQ media plane for freeq AV calls.
//!
//! Publishes mic Opus (+ optional camera H.264) to the SFU and plays remote
//! audio/video via `iroh-live` + `moq-native` — same stack as freeq-av /
//! freeq-sdk-ffi.
//!
//! Android: mic/speaker via cpal aaudio; local camera via Camera2
//! (`CameraCapture` Java helper in APK `classes.dex`) pushing NV12 into a
//! [`VideoSource`]. Remote H.264 decodes in-software (or MediaCodec when
//! available) when peers publish video.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh_live::media::{
    audio_backend::AudioBackend,
    codec::{AudioCodec, VideoCodec},
    format::{AudioPreset, VideoPreset},
    publish::LocalBroadcast,
    subscribe::RemoteBroadcast,
    traits::VideoSource,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::av::{
    broadcast_path, path_key, should_tap, MicLevel, VideoFrameStore, LOCAL_PREVIEW_KEY,
};

#[cfg(not(target_os = "android"))]
use iroh_live::media::{
    audio_backend::{AudioBackendOpts, DeviceId},
    capture::{CameraCapturer, CameraConfig, CameraSelector},
};

// ── Device enumeration (desktop) ───────────────────────────────────────────

/// One selectable capture/playback device for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDevice {
    /// Stable-ish key stored in prefs (`CameraInfo.id` or audio device name).
    pub id: String,
    /// Human label for combo boxes.
    pub name: String,
    pub is_default: bool,
}

/// Heuristic: virtual / loopback cameras (prefer real USB cams when falling back).
fn is_virtual_camera(name: &str, id: &str) -> bool {
    let s = format!("{name} {id}").to_ascii_lowercase();
    s.contains("virtual")
        || s.contains("obs")
        || s.contains("loopback")
        || s.contains("dummy")
        || s.contains("v4l2loopback")
}

/// List cameras. Hardware first; virtual (OBS/loopback) last on desktop.
/// On Android, front-facing cameras are listed first.
pub fn list_cameras() -> Vec<MediaDevice> {
    #[cfg(not(target_os = "android"))]
    {
        match CameraCapturer::list() {
            Ok(mut cams) => {
                cams.sort_by(|a, b| {
                    let av = is_virtual_camera(&a.name, &a.id);
                    let bv = is_virtual_camera(&b.name, &b.id);
                    av.cmp(&bv)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                cams.into_iter()
                    .map(|c| {
                        let label = if c.id.is_empty() || c.id == c.name {
                            c.name.clone()
                        } else {
                            format!("{} · {}", c.name, c.id)
                        };
                        MediaDevice {
                            id: c.id,
                            name: label,
                            is_default: false,
                        }
                    })
                    .collect()
            }
            Err(e) => {
                log::debug!("av-media: list cameras: {e}");
                Vec::new()
            }
        }
    }
    #[cfg(target_os = "android")]
    {
        crate::android_camera::list_cameras()
            .into_iter()
            .enumerate()
            .map(|(i, (id, name))| MediaDevice {
                id,
                name,
                is_default: i == 0,
            })
            .collect()
    }
}

/// ALSA plugin / exclusive-card names that fight PipeWire (dmix slave busy, or
/// exclusive `hw`/`sysdefault:CARD=` while WirePlumber already owns the card).
/// Prefer the native PipeWire host (or ALSA `pipewire` PCM) instead.
fn is_alsa_virtual_pcm(name: &str) -> bool {
    let n = name.trim();
    // Bare plugins.
    if matches!(
        n,
        "sysdefault"
            | "default"
            | "dmix"
            | "dsnoop"
            | "hw"
            | "plughw"
            | "null"
            | "pulse"
            | "jack"
            | "upmix"
            | "vdownmix"
            | "surround21"
            | "surround40"
            | "surround41"
            | "surround50"
            | "surround51"
            | "surround71"
            // ALSA→PipeWire bridge name; use system default / native PW host.
            | "pipewire"
    ) {
        return true;
    }
    // Card-qualified exclusive ALSA paths (e.g. sysdefault:CARD=C960 for EMEET).
    // Opening these bypasses PipeWire and fails when PW/OBS already hold the card.
    n.starts_with("sysdefault:")
        || n.starts_with("default:")
        || n.starts_with("dmix:")
        || n.starts_with("dsnoop:")
        || n.starts_with("front:")
        || n.starts_with("rear:")
        || n.starts_with("center_lfe:")
        || n.starts_with("side:")
        || n.starts_with("hw:")
        || n.starts_with("plughw:")
        || n.starts_with("surround")
        || n.starts_with("iec958:")
        || n.starts_with("hdmi:")
}

/// Drop persisted mic/speaker ids that are known-broken or redundant under
/// PipeWire so the UI shows "System default" (OS default source/sink — same as
/// the browser: currently the EMEET SmartCam mic when that is the default).
pub fn sanitize_audio_device_pref(id: Option<String>) -> Option<String> {
    id.filter(|s| !s.is_empty()).and_then(|s| {
        if is_alsa_virtual_pcm(&s) {
            None
        } else {
            Some(s)
        }
    })
}

/// Prefer the native PipeWire host when available (lists real sources like
/// "EMEET SmartCam C960 Mono" the same way Chromium does).
#[cfg(not(target_os = "android"))]
fn preferred_audio_host() -> Option<String> {
    let hosts = AudioBackend::available_hosts();
    if hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case("pipewire"))
    {
        Some("PipeWire".into())
    } else {
        None
    }
}

/// Sort for UI: system default first, then remaining real devices.
#[cfg(not(target_os = "android"))]
fn rank_audio_device(name: &str, is_default: bool) -> (u8, String) {
    let n = name.to_ascii_lowercase();
    let rank = if is_default {
        0
    } else if is_alsa_virtual_pcm(name) {
        9
    } else {
        5
    };
    (rank, n)
}

/// Names that are cpal PipeWire placeholders, stream monitors, or sinks
/// mis-listed as capture (Duplex sinks show up as inputs).
fn is_junk_capture_name(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() || n == "unknown" {
        return true;
    }
    matches!(
        n.as_str(),
        "default_input"
            | "default_output"
            | "default_sink"
            | "sink_default"
            | "input_default"
            | "output_default"
    ) || n.starts_with("alsa_output.")
        || n.contains("monitor of")
}

#[cfg(not(target_os = "android"))]
fn map_audio_devices(
    raw: impl IntoIterator<Item = iroh_live::media::audio_backend::AudioDevice>,
    inputs: bool,
) -> Vec<MediaDevice> {
    let mut devices: Vec<MediaDevice> = raw
        .into_iter()
        .filter(|d| !is_alsa_virtual_pcm(&d.name))
        .filter(|d| !is_junk_capture_name(&d.name))
        // Prefer unique names (cpal PW can list the same nick twice).
        .map(|d| MediaDevice {
            id: d.name.clone(),
            name: if d.is_default {
                format!("{} (system default)", d.name)
            } else {
                d.name
            },
            is_default: d.is_default,
        })
        .collect();
    // Dedupe by id, keep first (defaults sorted later).
    let mut seen = std::collections::HashSet::new();
    devices.retain(|d| seen.insert(d.id.clone()));
    devices.sort_by(|a, b| {
        rank_audio_device(&a.id, a.is_default).cmp(&rank_audio_device(&b.id, b.is_default))
    });
    let _ = inputs;
    devices
}

/// List microphones.
pub fn list_microphones() -> Vec<MediaDevice> {
    #[cfg(not(target_os = "android"))]
    {
        map_audio_devices(AudioBackend::list_inputs(), true)
    }
    #[cfg(target_os = "android")]
    {
        Vec::new()
    }
}

/// List speakers / output devices.
pub fn list_speakers() -> Vec<MediaDevice> {
    #[cfg(not(target_os = "android"))]
    {
        map_audio_devices(AudioBackend::list_outputs(), false)
    }
    #[cfg(target_os = "android")]
    {
        Vec::new()
    }
}

#[cfg(not(target_os = "android"))]
fn resolve_audio_device_id(name: Option<&str>, inputs: bool) -> Option<DeviceId> {
    let name = name.filter(|s| !s.is_empty())?;
    // ALSA virtual / exclusive PCMs (incl. bare "pipewire") → system default so
    // moq-media uses the native PipeWire host default (same as the browser).
    if is_alsa_virtual_pcm(name) {
        log::info!(
            "av-media: preferred audio {name:?} is an ALSA bridge/virtual PCM; \
             using system default (PipeWire when available)"
        );
        return None;
    }
    let list = if inputs {
        AudioBackend::list_inputs()
    } else {
        AudioBackend::list_outputs()
    };
    // Exact name first (avoid "default" substring matches), then case-insensitive
    // contains so "EMEET" matches "EMEET SmartCam C960 Mono". Skip junk/sinks.
    let name_l = name.to_ascii_lowercase();
    let usable: Vec<_> = list
        .into_iter()
        .filter(|d| !is_alsa_virtual_pcm(&d.name) && !is_junk_capture_name(&d.name))
        .collect();
    usable
        .iter()
        .find(|d| d.name == name)
        .or_else(|| {
            usable
                .iter()
                .find(|d| d.name.to_ascii_lowercase() == name_l)
        })
        .or_else(|| {
            usable
                .iter()
                .find(|d| d.name.to_ascii_lowercase().contains(&name_l))
        })
        .map(|d| d.id.clone())
}

// ── Config / session ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AvMediaConfig {
    pub sfu_url: url::Url,
    pub session_id: String,
    pub nick: String,
    pub instance: String,
    /// Initial mic mute (pre-call preference).
    pub muted: bool,
    /// Initial speaker mute — remote playback volume 0 (pre-call preference).
    pub speaker_muted: bool,
    /// Initial camera publish when hardware is available.
    pub camera_enabled: bool,
    /// Preferred camera id (`CameraInfo.id` / name). `None` = first available.
    pub camera_id: Option<String>,
    /// Preferred mic name. `None` = system default.
    pub mic_id: Option<String>,
    /// Preferred speaker name. `None` = system default.
    pub speaker_id: Option<String>,
}

/// Runtime controls from the UI thread (device switches, mute, camera).
#[derive(Debug, Clone)]
pub enum MediaControl {
    SetMuted(bool),
    /// Mute / unmute remote audio playback (speaker).
    SetSpeakerMuted(bool),
    SetCameraEnabled(bool),
    /// Re-open camera by id (`None` = default / first).
    SetCameraDevice(Option<String>),
    /// Switch mic by display name (`None` = system default).
    SetMicDevice(Option<String>),
    /// Switch speaker by display name (`None` = system default).
    SetSpeakerDevice(Option<String>),
}

/// Status updates from the media task.
#[derive(Debug, Clone)]
pub enum AvMediaUpdate {
    /// MoQ connected and publishing. `has_camera` is true when a capture
    /// device was opened and a video track is advertised.
    Live {
        video: VideoFrameStore,
        has_camera: bool,
        /// Live mic level (0..=1) written by the capture path.
        mic_level: MicLevel,
        /// True when a real capture device is feeding the Opus track.
        /// False = listen-only / silence publish (still advertises audio).
        has_mic: bool,
    },
    /// Session ended cleanly (stop / transport closed).
    Ended,
    /// Connect or runtime failure.
    Failed(String),
}

/// Background handle: drop or send on `stop` to tear down the MoQ session.
pub struct AvMediaSession {
    stop: Option<oneshot::Sender<()>>,
    control: Option<mpsc::UnboundedSender<MediaControl>>,
    pub muted: Arc<AtomicBool>,
    pub speaker_muted: Arc<AtomicBool>,
    pub camera_enabled: Arc<AtomicBool>,
    pub video: VideoFrameStore,
    pub mic_level: MicLevel,
    task: Option<tokio::task::JoinHandle<()>>,
    abort: Option<tokio::task::AbortHandle>,
}

impl AvMediaSession {
    pub fn start<F>(config: AvMediaConfig, on_status: F) -> Self
    where
        F: Fn(AvMediaUpdate) + Send + Sync + 'static,
    {
        let muted = Arc::new(AtomicBool::new(config.muted));
        let speaker_muted = Arc::new(AtomicBool::new(config.speaker_muted));
        // Respect pre-call camera pref; has_camera is reported after open.
        let camera_enabled = Arc::new(AtomicBool::new(config.camera_enabled));
        let video = VideoFrameStore::new();
        let mic_level = MicLevel::new();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
        let muted_task = muted.clone();
        let camera_task = camera_enabled.clone();
        let video_task = video.clone();
        let mic_level_task = mic_level.clone();
        let on_status = Arc::new(on_status);
        let task = tokio::spawn(async move {
            match run_media(
                config,
                muted_task,
                camera_task,
                video_task,
                mic_level_task,
                stop_rx,
                ctrl_rx,
                on_status.clone(),
            )
            .await
            {
                Ok(()) => on_status(AvMediaUpdate::Ended),
                Err(e) => on_status(AvMediaUpdate::Failed(e.to_string())),
            }
        });
        let abort = task.abort_handle();
        Self {
            stop: Some(stop_tx),
            control: Some(ctrl_tx),
            muted,
            speaker_muted,
            camera_enabled,
            video,
            mic_level,
            task: Some(task),
            abort: Some(abort),
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        if let Some(tx) = &self.control {
            let _ = tx.send(MediaControl::SetMuted(muted));
        }
    }

    pub fn set_speaker_muted(&self, muted: bool) {
        self.speaker_muted.store(muted, Ordering::Relaxed);
        if let Some(tx) = &self.control {
            let _ = tx.send(MediaControl::SetSpeakerMuted(muted));
        }
    }

    pub fn set_camera_enabled(&self, enabled: bool) {
        self.camera_enabled.store(enabled, Ordering::Relaxed);
        if let Some(tx) = &self.control {
            let _ = tx.send(MediaControl::SetCameraEnabled(enabled));
        }
    }

    pub fn set_camera_device(&self, id: Option<String>) {
        if let Some(tx) = &self.control {
            let _ = tx.send(MediaControl::SetCameraDevice(id));
        }
    }

    pub fn set_mic_device(&self, id: Option<String>) {
        if let Some(tx) = &self.control {
            let _ = tx.send(MediaControl::SetMicDevice(id));
        }
    }

    pub fn set_speaker_device(&self, id: Option<String>) {
        if let Some(tx) = &self.control {
            let _ = tx.send(MediaControl::SetSpeakerDevice(id));
        }
    }

    /// Request a clean stop (unpublish MoQ, drop audio devices). Prefer
    /// [`Self::stop_and_wait`] from async code so the task can finish teardown.
    pub fn request_stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        self.control.take();
    }

    /// Soft-stop then wait up to `timeout` for the media task to exit.
    /// Hard-aborts only if the task is still stuck — immediate abort skips
    /// MoQ unpublish and leaves **zombie broadcasts** on the SFU that peers
    /// may subscribe to (they see you online but hear silence).
    pub async fn stop_and_wait(&mut self, timeout: std::time::Duration) {
        self.request_stop();
        if let Some(task) = self.task.take() {
            match tokio::time::timeout(timeout, task).await {
                Ok(Ok(())) => log::info!("av-media: session task exited cleanly"),
                Ok(Err(e)) if e.is_cancelled() => {
                    log::debug!("av-media: session task cancelled");
                }
                Ok(Err(e)) => log::warn!("av-media: session task join: {e}"),
                Err(_) => {
                    log::warn!(
                        "av-media: session task still running after {timeout:?}; aborting \
                         (may leave a brief SFU ghost)"
                    );
                    if let Some(a) = self.abort.take() {
                        a.abort();
                    }
                }
            }
        }
        self.abort.take();
        self.video.clear();
        self.mic_level.clear();
    }

    /// Sync stop for Drop / non-async callers. Soft-stop + abort (best-effort).
    pub fn stop(&mut self) {
        self.request_stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.abort.take();
        self.video.clear();
        self.mic_level.clear();
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
    mic_level: MicLevel,
    mut stop: oneshot::Receiver<()>,
    mut control: mpsc::UnboundedReceiver<MediaControl>,
    on_status: Arc<dyn Fn(AvMediaUpdate) + Send + Sync>,
) -> Result<()> {
    // Watch so each remote audio track can react instantly to speaker mute.
    let (speaker_mute_tx, speaker_mute_rx) = watch::channel(config.speaker_muted);
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

    #[cfg(not(target_os = "android"))]
    let audio_backend = {
        let host = preferred_audio_host();
        let input_device = resolve_audio_device_id(config.mic_id.as_deref(), true);
        let output_device = resolve_audio_device_id(config.speaker_id.as_deref(), false);
        // Log what the OS thinks is available (helps confirm EMEET vs built-in).
        let inputs = list_microphones();
        let outputs = list_speakers();
        log::info!(
            "av-media: audio host={host:?} mic_pref={:?} → pinned={} | \
             speaker_pref={:?} → pinned={}",
            config.mic_id,
            input_device.is_some(),
            config.speaker_id,
            output_device.is_some()
        );
        if !inputs.is_empty() {
            log::info!(
                "av-media: microphones: {}",
                inputs
                    .iter()
                    .map(|d| {
                        if d.is_default {
                            format!("*{}", d.id)
                        } else {
                            d.id.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !outputs.is_empty() {
            log::info!(
                "av-media: speakers: {}",
                outputs
                    .iter()
                    .map(|d| {
                        if d.is_default {
                            format!("*{}", d.id)
                        } else {
                            d.id.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let ab = AudioBackend::new(AudioBackendOpts {
            host,
            input_device,
            output_device,
            ..Default::default()
        });
        ab.set_aec_enabled(false);
        ab
    };
    #[cfg(target_os = "android")]
    let audio_backend = {
        let ab = AudioBackend::default();
        ab.set_aec_enabled(false);
        ab
    };

    // Always publish an Opus track so peers see audio in the catalog.
    // Real mic when available; otherwise silence (listen-only). Prefer
    // falling back to the system default when a preferred device fails —
    // never leave the broadcast without audio (that looks like "not sending").
    log::info!(
        "av-media: outbound muted={} speaker_muted={} (peers hear silence while mic-muted; \
         we hear silence while speaker-muted)",
        muted.load(Ordering::Relaxed),
        *speaker_mute_rx.borrow()
    );
    let has_mic = match open_microphone(&audio_backend).await {
        Ok(mut mic) => {
            // Warm-up: wait until the capture ring actually has energy so we
            // don't advertise "mic open" while still InputNotReady→silence.
            let peak = warm_up_mic(&mut *mic, std::time::Duration::from_millis(800));
            log::info!(
                "av-media: mic warm-up peak={peak:.4} (0 = capture silent / not ready)"
            );
            if peak < 1e-5 {
                log::warn!(
                    "av-media: microphone opened but capture is silent so far — \
                     check PipeWire default source (EMEET), mute, and that OBS isn't \
                     exclusive; peers will hear silence until samples flow"
                );
            }
            let muteable = MuteableSource {
                inner: mic,
                muted: muted.clone(),
                level: mic_level.clone(),
                pulls: 0,
                voiced: 0,
                gain: 1.0,
                smooth_peak: 0.0,
            };
            match broadcast
                .audio()
                .set(muteable, AudioCodec::Opus, [AudioPreset::Hq])
            {
                Ok(()) => {
                    log::info!("av-media: microphone open, publishing Opus");
                    true
                }
                Err(e) => {
                    log::warn!("av-media: set mic audio source failed: {e}; publishing silence");
                    muted.store(true, Ordering::Relaxed);
                    if let Err(e2) = broadcast.audio().set(
                        MuteableSource::silence(muted.clone(), mic_level.clone()),
                        AudioCodec::Opus,
                        [AudioPreset::Hq],
                    ) {
                        log::error!("av-media: silence audio set also failed: {e2}");
                    }
                    false
                }
            }
        }
        Err(e) => {
            log::warn!(
                "av-media: no microphone ({e}); publishing silence (listen-only)"
            );
            muted.store(true, Ordering::Relaxed);
            if let Err(e2) = broadcast.audio().set(
                MuteableSource::silence(muted.clone(), mic_level.clone()),
                AudioCodec::Opus,
                [AudioPreset::Hq],
            ) {
                log::error!("av-media: silence audio set failed: {e2}");
            }
            false
        }
    };
    if !has_mic {
        log::info!("av-media: outbound audio is silence (listen-only / no capture)");
    }

    // Local-preview frame counter (diagnostics).
    let local_frame_count = Arc::new(AtomicU64::new(0));

    // Camera: desktop V4L2 / platform capture; Android Camera2 → NV12 push source.
    log::info!(
        "av-media: camera_enabled={} preferred_id={:?}",
        camera_enabled.load(Ordering::Relaxed),
        config.camera_id
    );
    // Keep Camera2 alive for the whole MoQ session (Android).
    #[cfg(target_os = "android")]
    let mut _android_camera_guard: Option<crate::android_camera::CameraCaptureGuard> = None;

    #[cfg(not(target_os = "android"))]
    let has_camera = {
        match open_camera_with_fallback(config.camera_id.as_deref()) {
            Ok((cam, opened_id)) => {
                let cam_name = cam.name().to_string();
                let gated = GatedCameraSource {
                    inner: cam,
                    enabled: camera_enabled.clone(),
                    preview: video_store.clone(),
                    frame_count: local_frame_count.clone(),
                };
                match broadcast
                    .video()
                    .set_source(gated, VideoCodec::H264, [VideoPreset::P360])
                {
                    Ok(()) => {
                        // Re-assert publish intent after a successful open. A prior
                        // failed dial may have stored false; join prefs say on.
                        if config.camera_enabled {
                            camera_enabled.store(true, Ordering::Relaxed);
                        }
                        log::info!(
                            "av-media: camera open id={opened_id:?} name={cam_name}, \
                             publishing H.264 360p (publish={})",
                            camera_enabled.load(Ordering::Relaxed)
                        );
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
        match open_android_camera(
            config.camera_id.as_deref(),
            &broadcast,
            camera_enabled.clone(),
            video_store.clone(),
            local_frame_count.clone(),
            config.camera_enabled,
        ) {
            Ok(guard) => {
                _android_camera_guard = Some(guard);
                true
            }
            Err(e) => {
                log::warn!("av-media: Android camera unavailable ({e}); audio-only");
                camera_enabled.store(false, Ordering::Relaxed);
                false
            }
        }
    };

    // Hold a LocalBroadcast::preview() track so SharedVideoSource stays unparked
    // even with no remote H.264 subscriber. GatedCameraSource also tees raw
    // frames into `__local__` on the capture thread; we additionally pump the
    // preview track into the store so self-view works even if the tee path
    // misses (rgba convert panic, gated race, etc.).
    let mut preview_keepalive = if has_camera {
        let t = broadcast.preview();
        if t.is_some() {
            log::info!("av-media: local preview keepalive held (SharedVideoSource unparked)");
        } else {
            log::warn!("av-media: broadcast.preview() returned None after set_source");
        }
        t
    } else {
        None
    };
    let _ = &mut preview_keepalive;

    // Second preview handle → dedicated pump into VideoFrameStore.
    let mut preview_pump: Option<std::thread::JoinHandle<()>> = if has_camera {
        spawn_local_preview_pump(&broadcast, video_store.clone(), camera_enabled.clone())
    } else {
        None
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

    log::info!(
        "av-media: MoQ connected, publishing {our_broadcast} has_mic={has_mic} has_camera={has_camera}"
    );
    on_status(AvMediaUpdate::Live {
        video: video_store.clone(),
        has_camera,
        mic_level: mic_level.clone(),
        has_mic,
    });

    let audio_for_playback = audio_backend.clone();
    let session_id = config.session_id.clone();
    let my_nick = config.nick.clone();
    let our_name = our_broadcast.clone();
    let store_for_subs = video_store.clone();
    // JoinSet so leave/stop aborts every remote tap (orphan spawns used to leak
    // AudioBackend + PipeWire streams across calls).
    let mut taps: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let mut tap_keys: std::collections::HashMap<String, tokio::task::AbortHandle> =
        std::collections::HashMap::new();

    // Keep broadcast + audio_backend alive for mid-call device switches.
    let _broadcast = broadcast;
    let audio_backend_ctrl = audio_backend;

    loop {
        tokio::select! {
            res = session_handle.closed() => {
                if let Err(e) = res {
                    log::info!("av-media: session closed: {e}");
                } else {
                    log::info!("av-media: session closed cleanly");
                }
                break;
            }
            _ = &mut stop => {
                log::info!("av-media: stop requested");
                break;
            }
            announce = sub_consumer.announced() => {
                let Some((path, announce)) = announce else {
                    log::info!("av-media: announce stream ended");
                    break;
                };
                match announce {
                    Some(broadcast_consumer) => {
                        let path_str = path.to_string();
                        if !should_tap(&path_str, &session_id, &our_name, &my_nick) {
                            continue;
                        }
                        // Replace any prior tap for this path.
                        if let Some(h) = tap_keys.remove(&path_str) {
                            h.abort();
                        }
                        log::info!("av-media: + remote {path_str}");
                        let ab = audio_for_playback.clone();
                        let ps = path_str.clone();
                        let store = store_for_subs.clone();
                        let key = path_key(&path_str).to_string();
                        let spk_mute = speaker_mute_rx.clone();
                        let handle = taps.spawn(async move {
                            tap_remote(ps, key, broadcast_consumer, ab, store, spk_mute).await;
                        });
                        tap_keys.insert(path_str, handle);
                    }
                    None => {
                        let path_str = path.to_string();
                        let key = path_key(&path_str).to_string();
                        store_for_subs.remove(&key);
                        if let Some(h) = tap_keys.remove(&path_str) {
                            h.abort();
                        }
                        log::info!("av-media: - remote {path_str}");
                    }
                }
            }
            // Reap finished taps so JoinSet doesn't grow forever.
            Some(res) = taps.join_next() => {
                if let Err(e) = res {
                    if !e.is_cancelled() {
                        log::debug!("av-media: tap task ended: {e}");
                    }
                }
            }
            msg = control.recv() => {
                let Some(msg) = msg else { break };
                match msg {
                    MediaControl::SetMuted(m) => {
                        muted.store(m, Ordering::Relaxed);
                        log::info!("av-media: muted={m}");
                    }
                    MediaControl::SetSpeakerMuted(m) => {
                        if speaker_mute_tx.send(m).is_ok() {
                            log::info!("av-media: speaker_muted={m}");
                        }
                    }
                    MediaControl::SetCameraEnabled(en) => {
                        camera_enabled.store(en, Ordering::Relaxed);
                        if !en {
                            video_store.remove(LOCAL_PREVIEW_KEY);
                        }
                    }
                    MediaControl::SetMicDevice(name) => {
                        #[cfg(not(target_os = "android"))]
                        {
                            let id = resolve_audio_device_id(name.as_deref(), true);
                            match audio_backend_ctrl.switch_input(id).await {
                                Ok(()) => log::info!("av-media: mic switched to {name:?}"),
                                Err(e) => log::warn!("av-media: switch mic failed: {e}"),
                            }
                        }
                        #[cfg(target_os = "android")]
                        {
                            let _ = name;
                        }
                    }
                    MediaControl::SetSpeakerDevice(name) => {
                        #[cfg(not(target_os = "android"))]
                        {
                            let id = resolve_audio_device_id(name.as_deref(), false);
                            match audio_backend_ctrl.switch_output(id).await {
                                Ok(()) => log::info!("av-media: speaker switched to {name:?}"),
                                Err(e) => log::warn!("av-media: switch speaker failed: {e}"),
                            }
                        }
                        #[cfg(target_os = "android")]
                        {
                            let _ = name;
                        }
                    }
                    MediaControl::SetCameraDevice(id) => {
                        // Drop old preview keepalive + pump before replacing source.
                        preview_keepalive = None;
                        if let Some(h) = preview_pump.take() {
                            // Track drop closes the pump; don't wait forever.
                            let _ = h;
                        }
                        video_store.remove(LOCAL_PREVIEW_KEY);
                        local_frame_count.store(0, Ordering::Relaxed);
                        #[cfg(not(target_os = "android"))]
                        {
                            match open_camera(id.as_deref()) {
                                Ok(cam) => {
                                    let name = cam.name().to_string();
                                    let gated = GatedCameraSource {
                                        inner: cam,
                                        enabled: camera_enabled.clone(),
                                        preview: video_store.clone(),
                                        frame_count: local_frame_count.clone(),
                                    };
                                    match _broadcast.video().set_source(
                                        gated,
                                        VideoCodec::H264,
                                        [VideoPreset::P360],
                                    ) {
                                        Ok(()) => {
                                            preview_keepalive = _broadcast.preview();
                                            preview_pump = spawn_local_preview_pump(
                                                &_broadcast,
                                                video_store.clone(),
                                                camera_enabled.clone(),
                                            );
                                            if is_virtual_camera(&name, id.as_deref().unwrap_or(""))
                                            {
                                                log::info!(
                                                    "av-media: using virtual camera {name} — \
                                                     in OBS: Controls → Start Virtual Camera \
                                                     (otherwise self-view is blank)"
                                                );
                                            }
                                            log::info!(
                                                "av-media: camera device switched to {id:?} ({name})"
                                            );
                                            on_status(AvMediaUpdate::Live {
                                                video: video_store.clone(),
                                                has_camera: true,
                                                mic_level: mic_level.clone(),
                                                has_mic,
                                            });
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "av-media: set_source after camera switch: {e}"
                                            );
                                            camera_enabled.store(false, Ordering::Relaxed);
                                            on_status(AvMediaUpdate::Live {
                                                video: video_store.clone(),
                                                has_camera: false,
                                                mic_level: mic_level.clone(),
                                                has_mic,
                                            });
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::warn!("av-media: open camera {id:?}: {e}");
                                }
                            }
                        }
                        #[cfg(target_os = "android")]
                        {
                            // Dropping the guard stops Camera2; then reopen on the new id.
                            // PushCameraSource stays registered on the broadcast.
                            drop(_android_camera_guard.take());
                            match crate::android_camera::start_capture(id.as_deref())
                                .and_then(|_| {
                                    crate::android_camera::wait_until_opened(
                                        std::time::Duration::from_secs(4),
                                    )
                                }) {
                                Ok(()) => {
                                    _android_camera_guard =
                                        Some(crate::android_camera::CameraCaptureGuard);
                                    preview_keepalive = _broadcast.preview();
                                    preview_pump = spawn_local_preview_pump(
                                        &_broadcast,
                                        video_store.clone(),
                                        camera_enabled.clone(),
                                    );
                                    log::info!(
                                        "av-media: Android camera switched to {id:?}"
                                    );
                                    on_status(AvMediaUpdate::Live {
                                        video: video_store.clone(),
                                        has_camera: true,
                                        mic_level: mic_level.clone(),
                                        has_mic,
                                    });
                                }
                                Err(e) => {
                                    log::warn!("av-media: Android camera switch {id:?}: {e}");
                                    camera_enabled.store(false, Ordering::Relaxed);
                                    on_status(AvMediaUpdate::Live {
                                        video: video_store.clone(),
                                        has_camera: false,
                                        mic_level: mic_level.clone(),
                                        has_mic,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    drop(session_handle);
    // Tear down every remote tap so AudioBackend / PW streams die with us.
    for (_, h) in tap_keys.drain() {
        h.abort();
    }
    taps.abort_all();
    while taps.join_next().await.is_some() {}
    // Drop preview tracks first so the pump thread sees is_closed and exits.
    drop(preview_keepalive);
    if let Some(h) = preview_pump.take() {
        // Best-effort join; don't block teardown if the pump is wedged.
        let _ = h.join();
    }
    drop(audio_backend_ctrl);
    drop(audio_for_playback);
    video_store.clear();
    mic_level.clear();
    // Brief yield so aborted tasks drop cpal/PW resources before the next dial.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    log::info!("av-media: teardown complete for {our_broadcast}");
    Ok(())
}

/// Subscribe audio + video for one remote broadcast until it ends or is aborted.
///
/// `key` is the frame-store id (`nick` or `nick~instance` from [`path_key`]).
///
/// Audio and video are independent: a catalog race that delays the audio
/// rendition must not tear down video (and vice versa). We wait with
/// `audio_ready` / `video_ready` and retry so late-advertised tracks still play.
///
/// `speaker_mute` drives remote playback volume (`0.0` when muted, `1.0` otherwise).
async fn tap_remote(
    path: String,
    key: String,
    broadcast_consumer: moq_lite::BroadcastConsumer,
    audio_backend: AudioBackend,
    video_store: VideoFrameStore,
    speaker_mute: watch::Receiver<bool>,
) {
    // Match freeq-sdk-ffi: tighter latency than the 150ms streaming default.
    let policy = iroh_live::media::playout::PlaybackPolicy::default()
        .with_max_latency(std::time::Duration::from_millis(60));
    let remote = match RemoteBroadcast::with_playback_policy(
        &path,
        broadcast_consumer,
        policy,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("av-media: catalog {path}: {e}");
            return;
        }
    };

    let mut audio_task = {
        let remote = remote.clone();
        let ab = audio_backend;
        let ps = path.clone();
        let mut speaker_mute = speaker_mute;
        tokio::spawn(async move {
            let mut consecutive_errs = 0u32;
            loop {
                match remote.audio_ready(&ab).await {
                    Ok(track) => {
                        consecutive_errs = 0;
                        log::info!("av-media: receiving audio from {ps}");
                        // Apply current speaker mute, then hold until the track
                        // ends or mute toggles (volume 0 = silence, keeps decode).
                        apply_speaker_volume(&track, *speaker_mute.borrow());
                        loop {
                            tokio::select! {
                                _ = track.stopped() => {
                                    log::info!("av-media: audio track ended for {ps}");
                                    break;
                                }
                                changed = speaker_mute.changed() => {
                                    if changed.is_err() {
                                        // Session tearing down — keep track until stop.
                                        track.stopped().await;
                                        break;
                                    }
                                    apply_speaker_volume(&track, *speaker_mute.borrow());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        consecutive_errs = consecutive_errs.saturating_add(1);
                        // Permanent peer/session death — don't spam every 500ms.
                        if msg.contains("dropped")
                            || msg.contains("closed")
                            || msg.contains("transport error")
                        {
                            log::warn!("av-media: audio sub {ps}: {e} (giving up)");
                            break;
                        }
                        if consecutive_errs <= 3 || consecutive_errs % 10 == 0 {
                            log::warn!(
                                "av-media: audio sub {ps}: {e} (retry {consecutive_errs})"
                            );
                        }
                        let backoff_ms = (500u64 * u64::from(consecutive_errs.min(8))).min(4_000);
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    }
                }
            }
        })
    };

    let mut video_task = {
        let remote = remote.clone();
        let store = video_store.clone();
        let key = key.clone();
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
                            store.set(key.clone(), w, h, bytes);
                        }
                        log::info!("av-media: video track ended for {path}");
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("dropped")
                            || msg.contains("closed")
                            || msg.contains("transport error")
                        {
                            log::debug!("av-media: video wait {path}: {e} (giving up)");
                            break;
                        }
                        log::debug!("av-media: video wait {path}: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        })
    };

    // Keep both pipelines alive until the remote broadcast closes.
    // Abort siblings — dropping JoinHandle alone detaches tasks and leaves
    // audio_ready retry loops logging "dropped" forever after transport death.
    tokio::select! {
        err = remote.closed() => {
            log::info!("av-media: remote closed {path}: {err}");
            audio_task.abort();
            video_task.abort();
        }
        _ = &mut audio_task => {
            log::debug!("av-media: audio task exited for {path}");
            video_task.abort();
        }
        _ = &mut video_task => {
            log::debug!("av-media: video task exited for {path}");
            audio_task.abort();
        }
    }
    video_store.remove(&key);
}

/// Set remote track volume from speaker-mute (`0.0` silence, `1.0` full).
fn apply_speaker_volume(track: &iroh_live::media::subscribe::AudioTrack, muted: bool) {
    track.set_volume(if muted { 0.0 } else { 1.0 });
}

/// Open the default mic input, retrying after clearing preferred devices if
/// the first open fails.
///
/// moq-media starts **output and input as a pair**. A bad speaker pref
/// (`sysdefault` busy under PipeWire) fails the whole pair, so we must clear
/// **output** as well as input before retrying.
async fn open_microphone(
    audio_backend: &AudioBackend,
) -> Result<Box<dyn iroh_live::media::traits::AudioSource>> {
    match audio_backend.default_input().await {
        Ok(mic) => Ok(Box::new(mic)),
        Err(first) => {
            log::warn!(
                "av-media: default_input failed ({first}); \
                 retry after clearing preferred I/O devices"
            );
            #[cfg(not(target_os = "android"))]
            {
                // Output first: start_cpal_streams requires a working speaker.
                if let Err(e) = audio_backend.switch_output(None).await {
                    log::warn!("av-media: switch_output(None) failed: {e}");
                }
                if let Err(e) = audio_backend.switch_input(None).await {
                    log::warn!("av-media: switch_input(None) failed: {e}");
                }
            }
            match audio_backend.default_input().await {
                Ok(mic) => Ok(Box::new(mic)),
                Err(e) => Err(e),
            }
        }
    }
}

/// Pull mic frames until we see energy or `budget` elapses. Returns peak abs.
fn warm_up_mic(
    mic: &mut dyn iroh_live::media::traits::AudioSource,
    budget: std::time::Duration,
) -> f32 {
    let deadline = std::time::Instant::now() + budget;
    let mut buf = vec![0.0f32; 960];
    let mut peak = 0.0f32;
    let mut ready = 0u32;
    while std::time::Instant::now() < deadline {
        match mic.pop_samples(&mut buf) {
            Ok(Some(n)) if n > 0 => {
                ready = ready.saturating_add(1);
                for &s in &buf[..n] {
                    peak = peak.max(s.abs());
                }
                if peak > 1e-3 {
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => {
                log::warn!("av-media: mic warm-up read error: {e:#}");
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    log::info!("av-media: mic warm-up ready_frames={ready} peak={peak:.4}");
    peak
}

/// Always-ready mono 48 kHz silence — keeps an Opus track advertised when
/// no capture device is available (listen-only join).
struct SilenceSource {
    format: iroh_live::media::format::AudioFormat,
}

impl Default for SilenceSource {
    fn default() -> Self {
        Self {
            format: iroh_live::media::format::AudioFormat::mono_48k(),
        }
    }
}

impl iroh_live::media::traits::AudioSource for SilenceSource {
    fn format(&self) -> iroh_live::media::format::AudioFormat {
        self.format
    }

    fn pop_samples(
        &mut self,
        buf: &mut [f32],
    ) -> anyhow::Result<Option<usize>> {
        for s in buf.iter_mut() {
            *s = 0.0;
        }
        Ok(Some(buf.len()))
    }
}

#[cfg(not(target_os = "android"))]
fn camera_config() -> CameraConfig {
    CameraConfig {
        selector: CameraSelector::TargetResolution(640, 360),
        preferred_format: None,
        // CPU RGBA — safer for local preview tee + software H.264.
        zero_copy: false,
    }
}

/// Open preferred camera, then fall back through the device list (hardware first).
///
/// Deliberately **does not** start/stop/probe frames here. V4L2 `dqbuf` is
/// blocking with no timeout — a probe that hangs leaves the device busy so
/// every later open fails (audio-only calls + blank self-view tile). Parent
/// behavior was `CameraCapturer::open` only; SharedVideoSource starts streaming
/// when the first preview/encoder subscriber arrives.
///
/// Open preferred camera, then fall back through hardware. Virtual (OBS) is
/// only used when the user **explicitly** preferred that id — never as a silent
/// fallback when the USB cam is busy (OBS often holds `/dev/video0` while
/// exposing `/dev/video10`).
#[cfg(not(target_os = "android"))]
fn open_camera_with_fallback(
    preferred: Option<&str>,
) -> Result<(Box<dyn VideoSource>, Option<String>)> {
    let config = camera_config();
    let preferred = preferred.filter(|s| !s.is_empty());

    let listed = match CameraCapturer::list() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("av-media: CameraCapturer::list failed: {e}");
            Vec::new()
        }
    };
    log::info!(
        "av-media: cameras available: {}",
        if listed.is_empty() {
            "(none)".into()
        } else {
            listed
                .iter()
                .map(|c| {
                    let v = if is_virtual_camera(&c.name, &c.id) {
                        " [virtual]"
                    } else {
                        ""
                    };
                    format!("{} ({}){v}", c.name, c.id)
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    let is_virt = |id: &str| {
        listed
            .iter()
            .find(|c| c.id == id || c.name == id)
            .map(|c| is_virtual_camera(&c.name, &c.id))
            .unwrap_or_else(|| is_virtual_camera(id, id))
    };

    let mut candidates: Vec<Option<String>> = Vec::new();

    // Explicit preference first (including virtual if the user picked OBS).
    if let Some(id) = preferred {
        candidates.push(Some(id.to_string()));
        if is_virt(id) {
            log::info!(
                "av-media: preferred {id} is virtual (OBS) — opening it first. \
                 USB cam may be busy because OBS is using it as a source."
            );
        }
    }

    // Then non-virtual hardware (skip virtuals for auto-pick).
    let mut cams = listed.clone();
    cams.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
    });
    for c in cams {
        if is_virtual_camera(&c.name, &c.id) {
            continue;
        }
        if c.supported_formats.is_empty() {
            continue;
        }
        if candidates.iter().any(|x| {
            x.as_deref() == Some(c.id.as_str()) || x.as_deref() == Some(c.name.as_str())
        }) {
            continue;
        }
        candidates.push(Some(c.id));
    }
    if !candidates.iter().any(|c| c.is_none()) {
        candidates.push(None);
    }

    let mut errors: Vec<String> = Vec::new();
    for cand in &candidates {
        let label = cand.as_deref().unwrap_or("(default)");
        match open_camera_with_busy_retry(cand.as_deref(), &config) {
            Ok(cam) => {
                let name = cam.name().to_string();
                // Reject *accidental* virtual from default open when user did not
                // prefer virtual.
                let user_wants_virtual = preferred.is_some_and(|p| is_virt(p));
                if is_virtual_camera(&name, label) && !user_wants_virtual {
                    log::warn!(
                        "av-media: rejecting auto-opened virtual camera {name} ({label})"
                    );
                    errors.push(format!("{label}: rejected virtual {name}"));
                    continue;
                }
                if is_virtual_camera(&name, label) {
                    log::info!(
                        "av-media: opened virtual camera {name} — ensure OBS has \
                         'Start Virtual Camera' enabled or self-view will be blank"
                    );
                }
                log::info!("av-media: opened camera id={label:?} name={name}");
                return Ok((cam, cand.clone()));
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("busy") {
                    log::warn!(
                        "av-media: open camera {label}: {msg} \
                         (often OBS or another app holds the USB cam)"
                    );
                } else {
                    log::warn!("av-media: open camera {label}: {msg}");
                }
                errors.push(format!("{label}: {e}"));
            }
        }
    }
    Err(anyhow::anyhow!(
        "no working camera ({})",
        errors.join(" | ")
    ))
}

/// Open one device; retry briefly on "busy" (race with OBS / previous session).
///
/// v4l2loopback nodes (OBS Virtual Camera) are routed to
/// [`crate::v4l2cam::V4l2MmapCapture`]: rusty-capture's dqbuf leaves the
/// `memory` field 0, which loopback drivers reject with EINVAL on the first
/// frame (silent blank self-view). Hardware cams keep rusty-capture.
#[cfg(not(target_os = "android"))]
fn open_camera_with_busy_retry(
    id: Option<&str>,
    config: &CameraConfig,
) -> Result<Box<dyn VideoSource>> {
    // Loopback devices get the dqbuf-fixed capturer (no busy-retry needed —
    // loopback nodes are multi-reader, "busy" doesn't apply the same way).
    #[cfg(target_os = "linux")]
    {
        // Resolve `(default)` to the first listed device so a loopback first
        // entry (e.g. OBS-only setups) also lands on the fixed capture path.
        let resolved: Option<String> = match id {
            Some(p) if p.starts_with("/dev/video") => Some(p.to_string()),
            Some(_) => None, // name, not a device path — rusty-capture handles
            None => CameraCapturer::list()
                .ok()
                .and_then(|c| c.into_iter().next())
                .map(|c| c.id)
                .filter(|p| p.starts_with("/dev/video")),
        };
        if let Some(path) = resolved {
            if crate::v4l2cam::is_loopback_device(&path) {
                let (w, h) = match config.selector {
                    CameraSelector::TargetResolution(w, h) => (w, h),
                    _ => (640, 360),
                };
                let cam = crate::v4l2cam::V4l2MmapCapture::open(&path, w, h)
                    .with_context(|| format!("loopback open {path}"))?;
                log::info!("av-media: using dqbuf-fixed capture for loopback {path}");
                return Ok(Box::new(cam));
            }
        }
    }

    const ATTEMPTS: u32 = 4;
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match CameraCapturer::open(None, id, config) {
            Ok(cam) => return Ok(Box::new(cam)),
            Err(e) => {
                let busy = e.to_string().to_ascii_lowercase().contains("busy");
                last = Some(e);
                if busy && attempt + 1 < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(150 * (attempt + 1) as u64));
                    continue;
                }
                break;
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("open failed")))
}

/// Luma (Rec.601 approx) min/max across RGBA bytes — used by live diagnostics
/// and tests to detect a real camera picture vs a blank/uniform buffer.
/// Ships with the crate (not test-only) so pump/tee logs report real content.
pub(crate) fn rgba_luma_range(rgba: &[u8]) -> (u8, u8) {
    let mut min = 255u8;
    let mut max = 0u8;
    for px in rgba.chunks_exact(4) {
        let l = ((77u32 * px[0] as u32 + 150u32 * px[1] as u32 + 29u32 * px[2] as u32) >> 8) as u8;
        min = min.min(l);
        max = max.max(l);
    }
    (min, max)
}

/// Pump `broadcast.preview()` frames into `__local__` for self-view.
fn spawn_local_preview_pump(
    broadcast: &LocalBroadcast,
    store: VideoFrameStore,
    enabled: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    let mut track = match broadcast.preview() {
        Some(t) => t,
        None => {
            log::warn!("av-media: preview() for pump returned None");
            return None;
        }
    };
    match std::thread::Builder::new()
        .name("local-preview-pump".into())
        .spawn(move || {
            let mut logged = false;
            let mut pump_waited_ms = 0u64;
            let mut warned_no_frames = false;
            loop {
                if !enabled.load(Ordering::Relaxed) {
                    store.remove(LOCAL_PREVIEW_KEY);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                if let Some(frame) = track.try_recv() {
                    let (w, h) = (frame.width(), frame.height());
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let rgba = frame.rgba_image();
                        let bytes: Arc<[u8]> = rgba.as_raw().as_slice().into();
                        (w, h, bytes)
                    })) {
                        Ok((w, h, bytes)) => {
                            let (r0, g0, b0, a0) = bytes
                                .get(0..4)
                                .map(|p| (p[0], p[1], p[2], p[3]))
                                .unwrap_or((0, 0, 0, 0));
                            let (luma_min, luma_max) = rgba_luma_range(&bytes);
                            store.set(LOCAL_PREVIEW_KEY, w, h, bytes);
                            if !logged {
                                logged = true;
                                log::info!(
                                    "av-media: local preview pump first frame {w}x{h} \
                                     rgba0=({r0},{g0},{b0},{a0}) luma={luma_min}..{luma_max}"
                                );
                                if luma_max == luma_min {
                                    log::warn!(
                                        "av-media: local preview frame is uniform \
                                         (blank capture?) — OBS Virtual Camera not started?"
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            log::warn!(
                                "av-media: local preview pump rgba_image panicked {w}x{h}"
                            );
                        }
                    }
                } else if track.is_closed() {
                    log::info!("av-media: local preview pump track closed");
                    break;
                } else {
                    // Warn once if capture is up but never yields a frame
                    // (dead OBS virtual node, busy device, etc.).
                    if !logged && pump_waited_ms >= 3_000 && !warned_no_frames {
                        warned_no_frames = true;
                        log::warn!(
                            "av-media: no local preview frames after {}ms — camera capture \
                             is not producing (OBS Virtual Camera not started, or device busy)",
                            pump_waited_ms
                        );
                    }
                    pump_waited_ms = pump_waited_ms.saturating_add(10);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }) {
        Ok(h) => {
            log::info!("av-media: local preview pump thread started");
            Some(h)
        }
        Err(e) => {
            log::warn!("av-media: local preview pump spawn failed: {e}");
            None
        }
    }
}

/// Mid-call device switch uses the same fallback path.
#[cfg(not(target_os = "android"))]
fn open_camera(id: Option<&str>) -> Result<Box<dyn VideoSource>> {
    open_camera_with_fallback(id).map(|(c, _)| c)
}

/// Start Android Camera2, register a gated NV12 [`VideoSource`], and return a
/// guard that stops the camera when the MoQ session ends.
#[cfg(target_os = "android")]
fn open_android_camera(
    camera_id: Option<&str>,
    broadcast: &LocalBroadcast,
    camera_enabled: Arc<AtomicBool>,
    video_store: VideoFrameStore,
    local_frame_count: Arc<AtomicU64>,
    want_publish: bool,
) -> Result<crate::android_camera::CameraCaptureGuard> {
    crate::android_camera::start_capture(camera_id)
        .context("CameraCapture.start")?;
    // Camera2 open + session configure is async on a Java handler thread.
    crate::android_camera::wait_until_opened(std::time::Duration::from_secs(5))
        .context("Camera2 session")?;

    let label = camera_id
        .filter(|s| !s.is_empty())
        .unwrap_or("front")
        .to_string();
    let cam = crate::android_camera::PushCameraSource::new(format!("android-camera:{label}"));
    let gated = GatedCameraSource {
        inner: Box::new(cam),
        enabled: camera_enabled.clone(),
        preview: video_store,
        frame_count: local_frame_count,
    };
    broadcast
        .video()
        .set_source(gated, VideoCodec::H264, [VideoPreset::P360])
        .context("video set_source")?;

    if want_publish {
        camera_enabled.store(true, Ordering::Relaxed);
    }
    log::info!(
        "av-media: Android camera open id={camera_id:?}, publishing H.264 360p (publish={})",
        camera_enabled.load(Ordering::Relaxed)
    );
    Ok(crate::android_camera::CameraCaptureGuard)
}

/// Wraps camera capture: when disabled, drain frames but publish none so the
/// H.264 track stays in the catalog. Always tees enabled frames into
/// `__local__` for self-view (requires a SharedVideoSource subscriber — we
/// hold `broadcast.preview()` as keepalive).
///
/// `inner` is boxed so v4l2loopback devices (OBS Virtual Camera) can use the
/// loopback-safe [`crate::v4l2cam::V4l2MmapCapture`] instead of rusty-capture's
/// `CameraCapturer` (whose v4l2r dqbuf leaves `memory=0` → EINVAL on loopback).
/// On Android, `inner` is the Camera2 [`PushCameraSource`].
struct GatedCameraSource {
    inner: Box<dyn VideoSource>,
    enabled: Arc<AtomicBool>,
    preview: VideoFrameStore,
    frame_count: Arc<AtomicU64>,
}

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
        let frame = match self.inner.pop_frame() {
            Ok(f) => f,
            Err(e) => {
                // Surface real capture failures (e.g. OBS Virtual Camera not
                // started → v4l2loopback dqbuf EINVAL; busy USB cam → EBUSY).
                // Upstream SharedVideoSource stops the capture thread silently
                // on Err — that left users staring at a blank tile with no log.
                let msg = format!("{e:#}");
                let already = self.frame_count.load(Ordering::Relaxed) > 0;
                let key = if msg.contains("EINVAL") {
                    "virtual camera not producing (start OBS Virtual Camera output?)"
                } else if msg.contains("EBUSY") || msg.contains("busy") {
                    "camera busy (held by OBS or another app?)"
                } else {
                    "capture error"
                };
                log::warn!(
                    "av-media: camera {} pop_frame failed: {msg} — {key} {}",
                    self.inner.name(),
                    if already {
                        "(frames had been flowing)"
                    } else {
                        "(no frames ever captured)"
                    }
                );
                self.preview.remove(LOCAL_PREVIEW_KEY);
                return Err(e);
            }
        };
        if !self.enabled.load(Ordering::Relaxed) {
            // Drain so the capture pipeline doesn't back up; hide self-view.
            // Peers still have the video track in the catalog but get no frames
            // until the camera is turned back on.
            self.preview.remove(LOCAL_PREVIEW_KEY);
            let n = self.frame_count.load(Ordering::Relaxed);
            if n > 0 && n % 300 == 0 {
                log::debug!("av-media: camera gated off (publish idle), drained={n}");
            }
            return Ok(None);
        }
        if let Some(ref f) = frame {
            let (w, h) = (f.width(), f.height());
            // Best-effort local preview — never fail the encode path.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rgba = f.rgba_image();
                let bytes: Arc<[u8]> = rgba.as_raw().as_slice().into();
                (w, h, bytes)
            })) {
                Ok((w, h, bytes)) => {
                    // Sample first pixel + luma range for blank/alpha diagnostics.
                    let (r0, g0, b0, a0) = bytes
                        .get(0..4)
                        .map(|p| (p[0], p[1], p[2], p[3]))
                        .unwrap_or((0, 0, 0, 0));
                    let (luma_min, luma_max) = rgba_luma_range(&bytes);
                    self.preview.set(LOCAL_PREVIEW_KEY, w, h, bytes);
                    let n = self.frame_count.fetch_add(1, Ordering::Relaxed);
                    if n == 0 {
                        log::info!(
                            "av-media: first published/preview frame {w}x{h} \
                             rgba0=({r0},{g0},{b0},{a0}) luma={luma_min}..{luma_max} \
                             (outbound video live)"
                        );
                        if luma_max == luma_min {
                            log::warn!(
                                "av-media: published frame is uniform (blank capture?) — \
                                 OBS Virtual Camera not started?"
                            );
                        }
                    } else if n == 30 || n == 300 || n == 3000 {
                        log::info!(
                            "av-media: published frames={n} ({w}x{h}) \
                             rgba0=({r0},{g0},{b0},{a0}) luma={luma_min}..{luma_max}"
                        );
                    }
                }
                Err(_) => {
                    log::warn!("av-media: rgba_image panicked on local frame {w}x{h}");
                    // Still count as a publishable frame even if preview tee failed.
                    let n = self.frame_count.fetch_add(1, Ordering::Relaxed);
                    if n == 0 {
                        log::info!("av-media: first publish frame {w}x{h} (preview tee failed)");
                    }
                }
            }
        }
        Ok(frame)
    }
}

/// RMS of a PCM buffer — used by tests and as a simple energy metric.
pub fn pcm_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Continuous 48 kHz mono sine — freeq mesh rate (see freeq-av `SPEAK_RATE`).
///
/// Used by interop tests as a deterministic non-silent capture substitute so
/// we prove Opus encode/decode energy without requiring a real microphone.
struct ToneSource {
    format: iroh_live::media::format::AudioFormat,
    phase: f32,
    frequency: f32,
}

impl ToneSource {
    fn hz440() -> Self {
        Self {
            format: iroh_live::media::format::AudioFormat::mono_48k(),
            phase: 0.0,
            frequency: 440.0,
        }
    }
}

impl iroh_live::media::traits::AudioSource for ToneSource {
    fn format(&self) -> iroh_live::media::format::AudioFormat {
        self.format
    }

    fn pop_samples(
        &mut self,
        buf: &mut [f32],
    ) -> anyhow::Result<Option<usize>> {
        let channels = self.format.channel_count.max(1) as usize;
        let frames = buf.len() / channels;
        let phase_inc = self.frequency / self.format.sample_rate as f32;
        for i in 0..frames {
            let sample = (2.0 * std::f32::consts::PI * self.phase).sin() * 0.5;
            for ch in 0..channels {
                buf[i * channels + ch] = sample;
            }
            self.phase += phase_inc;
            self.phase -= self.phase.floor();
        }
        // Always a full buffer — never `None` (iroh-live encoder skips `None`).
        Ok(Some(buf.len()))
    }
}

/// Target peak after AGC (linear). Browsers apply getUserMedia AGC; cpal/PW
/// does not — EMEET often lands at peak ~0.005 which Opus/bots treat as silence.
const AGC_TARGET_PEAK: f32 = 0.28;
/// Cap boost so noise floor alone doesn't become roar (≈ +32 dB).
const AGC_MAX_GAIN: f32 = 40.0;
/// Don't boost pure digital silence / inactive rings.
const AGC_MIN_PEAK: f32 = 5e-5;
/// Speech-ish energy after gain (for diagnostics).
const VOICED_PEAK: f32 = 0.02;

/// Wraps an AudioSource and emits silence while muted (keeps the Opus track live).
///
/// Mic level is measured on the **post-AGC** samples before mute zeros them.
///
/// When the inner source returns `None` (cpal ring not ready yet), we still
/// hand the encoder a full silence frame. The iroh-live encoder skips ticks
/// on `None`, which means **no Opus packets go out** — peers hear nothing
/// until the ring becomes ready, and intermittent `None`s produce dropouts.
/// Padding matches freeq-sdk-ffi `PushAudioSource` (always `Some(buf.len())`).
///
/// Short `Some(n)` reads are also padded to a full buffer so underruns never
/// starve the Opus encoder of continuous frames.
struct MuteableSource {
    inner: Box<dyn iroh_live::media::traits::AudioSource>,
    muted: Arc<AtomicBool>,
    level: MicLevel,
    /// Encode pulls (each ~20ms). Used for sparse diagnostics.
    pulls: u64,
    /// Pulls that had post-AGC energy above [`VOICED_PEAK`].
    voiced: u64,
    /// Adaptive linear gain (smoothed).
    gain: f32,
    /// Smoothed pre-gain peak for AGC.
    smooth_peak: f32,
}

impl MuteableSource {
    fn silence(muted: Arc<AtomicBool>, level: MicLevel) -> Self {
        Self {
            inner: Box::new(SilenceSource::default()),
            muted,
            level,
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        }
    }

    fn apply_agc(&mut self, buf: &mut [f32], pre_peak: f32) -> f32 {
        // Slow envelope so gain doesn't pump on every syllable.
        self.smooth_peak = self.smooth_peak * 0.92 + pre_peak * 0.08;
        if self.smooth_peak >= AGC_MIN_PEAK {
            let desired = (AGC_TARGET_PEAK / self.smooth_peak).clamp(1.0, AGC_MAX_GAIN);
            self.gain = self.gain * 0.9 + desired * 0.1;
        } else {
            // Decay gain when capture is dead so we don't explode on first sample.
            self.gain = (self.gain * 0.95).max(1.0);
        }
        let g = self.gain;
        let mut post_peak = 0.0f32;
        for s in buf.iter_mut() {
            let v = (*s * g).clamp(-0.95, 0.95);
            *s = v;
            post_peak = post_peak.max(v.abs());
        }
        post_peak
    }
}

impl iroh_live::media::traits::AudioSource for MuteableSource {
    fn format(&self) -> iroh_live::media::format::AudioFormat {
        self.inner.format()
    }

    fn pop_samples(
        &mut self,
        buf: &mut [f32],
    ) -> anyhow::Result<Option<usize>> {
        let mut pre_peak = 0.0f32;
        let n = match self.inner.pop_samples(buf)? {
            Some(n) if n > 0 => {
                let take = n.min(buf.len());
                for &s in &buf[..take] {
                    pre_peak = pre_peak.max(s.abs());
                }
                // Pad remainder before AGC so we gain a full encoder frame.
                for s in &mut buf[take..] {
                    *s = 0.0;
                }
                let post = self.apply_agc(buf, pre_peak);
                self.level.observe(buf);
                let _ = post;
                buf.len()
            }
            Some(_) | None => {
                // Capture underrun / not-ready / empty: keep the track alive.
                for s in buf.iter_mut() {
                    *s = 0.0;
                }
                self.level.observe(&[]);
                self.smooth_peak *= 0.9;
                buf.len()
            }
        };
        let mut post_peak = 0.0f32;
        for &s in buf.iter() {
            post_peak = post_peak.max(s.abs());
        }
        self.pulls = self.pulls.saturating_add(1);
        if post_peak > VOICED_PEAK {
            self.voiced = self.voiced.saturating_add(1);
        }
        let is_muted = self.muted.load(Ordering::Relaxed);
        if is_muted {
            for s in buf.iter_mut() {
                *s = 0.0;
            }
        }
        // Sparse log: first encode pull, then every ~5s (250 * 20ms).
        if self.pulls == 1 || self.pulls % 250 == 0 {
            log::info!(
                "av-media: outbound encode pulls={} voiced={} muted={} \
                 pre_peak={pre_peak:.4} post_peak={post_peak:.4} gain={:.1}x",
                self.pulls,
                self.voiced,
                is_muted,
                self.gain
            );
        }
        // Always `Some(full)` — continuous Opus packets while the call is live.
        Ok(Some(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_live::media::codec::AudioCodec;
    use iroh_live::media::format::{AudioFormat, AudioPreset};
    use iroh_live::media::playout::PlaybackPolicy;
    use iroh_live::media::publish::LocalBroadcast;
    use iroh_live::media::subscribe::RemoteBroadcast;
    use iroh_live::media::traits::{
        AudioSink, AudioSinkHandle, AudioSource, AudioStreamFactory,
    };
    use n0_future::boxed::BoxFuture;
    use std::time::Duration;

    /// Real-device check (Linux): open a camera with the same capture crate the
    /// app uses, pull frames, assert non-uniform content. Uses
    /// `SLEEK_TEST_CAMERA_ID` (e.g. `/dev/video10`) when set, else default.
    /// Skips with a printed reason when the device is busy or absent.
    #[cfg(all(test, not(target_os = "android")))]
    #[test]
    fn real_camera_capture_produces_nonuniform_frames() {
        let id = std::env::var("SLEEK_TEST_CAMERA_ID").ok();
        let Ok(mut cam) = CameraCapturer::open(None, id.as_deref(), &camera_config()) else {
            eprintln!("SKIP real_camera id={id:?}: open failed (busy/absent)");
            return;
        };
        if let Err(e) = cam.start() {
            eprintln!("SKIP real_camera id={id:?}: start failed: {e}");
            return;
        }
        let deadline = std::time::Instant::now() + Duration::from_millis(2_500);
        let mut frame = None;
        while std::time::Instant::now() < deadline {
            match cam.pop_frame() {
                Ok(Some(f)) => {
                    frame = Some(f);
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    let _ = cam.stop();
                    eprintln!("SKIP real_camera id={id:?}: pop_frame error: {e}");
                    return;
                }
            }
        }
        let _ = cam.stop();
        let Some(f) = frame else {
            eprintln!(
                "SKIP real_camera id={id:?}: no frames (OBS Virtual Camera not started?)"
            );
            return;
        };
        let (w, h) = (f.width(), f.height());
        let rgba = f.rgba_image();
        let bytes = rgba.as_raw().as_slice();
        let (min, max) = rgba_luma_range(bytes);
        let (r0, g0, b0, a0) = bytes
            .get(0..4)
            .map(|p| (p[0], p[1], p[2], p[3]))
            .unwrap_or((0, 0, 0, 0));
        eprintln!(
            "camera id={id:?} {w}x{h} rgba0=({r0},{g0},{b0},{a0}) luma range {min}..{max}"
        );
        assert!(
            max > min,
            "real camera frame must be non-uniform (luma range {min}..{max}); \
             got a blank/uniform buffer"
        );
    }

    /// Source that always returns `None` — models cpal InputNotReady.
    struct AlwaysNone;

    impl AudioSource for AlwaysNone {
        fn format(&self) -> AudioFormat {
            AudioFormat::mono_48k()
        }
        fn pop_samples(&mut self, _buf: &mut [f32]) -> anyhow::Result<Option<usize>> {
            Ok(None)
        }
    }

    /// Source that returns short buffers (partial read underrun).
    struct ShortRead {
        n: usize,
    }

    impl AudioSource for ShortRead {
        fn format(&self) -> AudioFormat {
            AudioFormat::mono_48k()
        }
        fn pop_samples(&mut self, buf: &mut [f32]) -> anyhow::Result<Option<usize>> {
            let n = self.n.min(buf.len());
            for s in &mut buf[..n] {
                *s = 0.25;
            }
            Ok(Some(n))
        }
    }

    #[test]
    fn muteable_source_never_returns_none_on_underrun() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut src = MuteableSource {
            inner: Box::new(AlwaysNone),
            muted,
            level: MicLevel::new(),
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        };
        let mut buf = [1.0f32; 960]; // 20ms @ 48k
        for _ in 0..50 {
            let n = src.pop_samples(&mut buf).unwrap();
            assert_eq!(n, Some(buf.len()), "encoder must get continuous frames");
            assert!(
                buf.iter().all(|&s| s == 0.0),
                "underrun padding must be silence"
            );
        }
    }

    #[test]
    fn muteable_source_pads_short_reads_to_full_buffer() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut src = MuteableSource {
            inner: Box::new(ShortRead { n: 100 }),
            muted,
            level: MicLevel::new(),
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        };
        let mut buf = [0.0f32; 960];
        let n = src.pop_samples(&mut buf).unwrap();
        assert_eq!(n, Some(960));
        // AGC may boost above 0.25; pad region stays 0.
        assert!(
            buf[..100].iter().all(|&s| s > 0.2),
            "short-read region should keep signal (with AGC)"
        );
        assert!(buf[100..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn muteable_source_zeros_when_muted_but_keeps_frames() {
        let muted = Arc::new(AtomicBool::new(true));
        let mut src = MuteableSource {
            inner: Box::new(ToneSource::hz440()),
            muted,
            level: MicLevel::new(),
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        };
        let mut buf = [0.0f32; 480];
        let n = src.pop_samples(&mut buf).unwrap();
        assert_eq!(n, Some(buf.len()));
        assert_eq!(pcm_rms(&buf), 0.0, "muted must be silence");
        // Level still saw pre-mute energy from the tone.
        assert!(
            src.level.get() > 0.01,
            "mic meter should move while muted"
        );
    }

    #[test]
    fn muteable_source_tone_has_energy_when_unmuted() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut src = MuteableSource {
            inner: Box::new(ToneSource::hz440()),
            muted,
            level: MicLevel::new(),
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        };
        let mut buf = [0.0f32; 4800]; // 100ms
        let n = src.pop_samples(&mut buf).unwrap();
        assert_eq!(n, Some(buf.len()));
        let rms = pcm_rms(&buf);
        assert!(
            rms > 0.1,
            "unmuted tone RMS {rms} should be clearly non-silent"
        );
    }

    #[test]
    fn silence_source_is_continuous_zeros() {
        let mut s = SilenceSource::default();
        let mut buf = [1.0f32; 320];
        assert_eq!(s.pop_samples(&mut buf).unwrap(), Some(320));
        assert!(buf.iter().all(|&x| x == 0.0));
    }

    /// freeq-av-style tap: capture decoded PCM instead of playing it.
    struct TapBackend {
        tx: std::sync::mpsc::SyncSender<Vec<f32>>,
    }

    impl AudioStreamFactory for TapBackend {
        fn create_input(
            &self,
            format: AudioFormat,
        ) -> BoxFuture<anyhow::Result<Box<dyn AudioSource>>> {
            let src = SilenceSource {
                format: if format.sample_rate == 0 {
                    AudioFormat::mono_48k()
                } else {
                    format
                },
            };
            Box::pin(async move { Ok(Box::new(src) as Box<dyn AudioSource>) })
        }

        fn create_output(
            &self,
            format: AudioFormat,
        ) -> BoxFuture<anyhow::Result<Box<dyn AudioSink>>> {
            let tx = self.tx.clone();
            Box::pin(async move {
                Ok(Box::new(TapSink {
                    format,
                    paused: Arc::new(AtomicBool::new(false)),
                    tx,
                }) as Box<dyn AudioSink>)
            })
        }
    }

    struct TapSink {
        format: AudioFormat,
        paused: Arc<AtomicBool>,
        tx: std::sync::mpsc::SyncSender<Vec<f32>>,
    }

    impl AudioSinkHandle for TapSink {
        fn cloned_boxed(&self) -> Box<dyn AudioSinkHandle> {
            Box::new(TapSinkHandle {
                paused: self.paused.clone(),
            })
        }
        fn pause(&self) {
            self.paused.store(true, Ordering::Relaxed);
        }
        fn resume(&self) {
            self.paused.store(false, Ordering::Relaxed);
        }
        fn is_paused(&self) -> bool {
            self.paused.load(Ordering::Relaxed)
        }
        fn toggle_pause(&self) {
            let _ = self
                .paused
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(!v));
        }
    }

    impl AudioSink for TapSink {
        fn format(&self) -> anyhow::Result<AudioFormat> {
            Ok(self.format)
        }
        fn push_samples(&mut self, buf: &[f32]) -> anyhow::Result<()> {
            let _ = self.tx.try_send(buf.to_vec());
            Ok(())
        }
        fn handle(&self) -> Box<dyn AudioSinkHandle> {
            Box::new(TapSinkHandle {
                paused: self.paused.clone(),
            })
        }
    }

    struct TapSinkHandle {
        paused: Arc<AtomicBool>,
    }

    impl AudioSinkHandle for TapSinkHandle {
        fn cloned_boxed(&self) -> Box<dyn AudioSinkHandle> {
            Box::new(TapSinkHandle {
                paused: self.paused.clone(),
            })
        }
        fn pause(&self) {
            self.paused.store(true, Ordering::Relaxed);
        }
        fn resume(&self) {
            self.paused.store(false, Ordering::Relaxed);
        }
        fn is_paused(&self) -> bool {
            self.paused.load(Ordering::Relaxed)
        }
        fn toggle_pause(&self) {
            let _ = self
                .paused
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(!v));
        }
    }

    /// Publish through the shipped MuteableSource → LocalBroadcast Opus path,
    /// subscribe like freeq-av/eve (RemoteBroadcast + tap sink), assert energy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn outbound_muteable_tone_is_audible_to_subscriber() {
        let path = broadcast_path("01TESTSESS", "desktop", "a1b2c3d4");
        assert_eq!(path, "01TESTSESS/desktop~a1b2c3d4");

        let broadcast = LocalBroadcast::new();
        let muted = Arc::new(AtomicBool::new(false));
        let muteable = MuteableSource {
            inner: Box::new(ToneSource::hz440()),
            muted,
            level: MicLevel::new(),
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        };
        broadcast
            .audio()
            .set(muteable, AudioCodec::Opus, [AudioPreset::Hq])
            .expect("set Opus source (shipped publish path)");

        let consumer = broadcast.consume();
        // Keep producer alive for the duration of the test.
        let _keepalive = broadcast;

        let remote = RemoteBroadcast::with_playback_policy(
            &path,
            consumer,
            PlaybackPolicy::unmanaged(),
        )
        .await
        .expect("catalog from LocalBroadcast");

        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(64);
        let backend = TapBackend { tx };
        let _track = remote
            .audio_ready(&backend)
            .await
            .expect("audio_ready — catalog must advertise Opus");

        // Collect decoded PCM for ~1.5s of wall time (Opus frame cadence).
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        let mut samples: Vec<f32> = Vec::new();
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => samples.extend_from_slice(&chunk),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if samples.len() > 48_000 / 2 {
                break; // ~0.5s mono 48k is enough for RMS
            }
        }

        assert!(
            !samples.is_empty(),
            "subscriber got no PCM — publish path not producing Opus frames"
        );
        let rms = pcm_rms(&samples);
        assert!(
            rms > 0.01,
            "decoded RMS {rms:.6} too low (samples={}) — bot would hear silence",
            samples.len()
        );
    }

    /// Catalog-only silence when capture is missing still produces frames
    /// (listen-only), so agents stay attached rather than seeing no track.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn silence_source_still_publishes_continuous_opus_track() {
        let broadcast = LocalBroadcast::new();
        let muted = Arc::new(AtomicBool::new(true));
        let muteable = MuteableSource {
            inner: Box::new(SilenceSource::default()),
            muted,
            level: MicLevel::new(),
            pulls: 0,
            voiced: 0,
            gain: 1.0,
            smooth_peak: 0.0,
        };
        broadcast
            .audio()
            .set(muteable, AudioCodec::Opus, [AudioPreset::Hq])
            .expect("silence Opus set");
        let consumer = broadcast.consume();
        let _keepalive = broadcast;

        let remote = RemoteBroadcast::with_playback_policy(
            "sess/listen-only",
            consumer,
            PlaybackPolicy::unmanaged(),
        )
        .await
        .expect("catalog");

        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(32);
        let backend = TapBackend { tx };
        let _track = remote.audio_ready(&backend).await.expect("audio track");

        let deadline = std::time::Instant::now() + Duration::from_millis(800);
        let mut got = 0usize;
        while std::time::Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(40)) {
                got += chunk.len();
                if got > 2000 {
                    break;
                }
            }
        }
        assert!(
            got > 0,
            "listen-only silence must still emit continuous frames (got 0 samples)"
        );
    }

}
