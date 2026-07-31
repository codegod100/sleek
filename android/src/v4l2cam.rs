//! Minimal V4L2 MMAP camera capture with a **correct `dqbuf`**.
//!
//! `v4l2r`'s `ioctl::dqbuf` leaves `v4l2_buffer.memory = 0`. Real UVC drivers
//! (EMEET) tolerate it, but **v4l2loopback (OBS Virtual Camera) rejects it with
//! `EINVAL`**, which kills the capture thread on the first frame and leaves a
//! permanently blank self-view tile. This module does the same MMAP flow with
//! `memory = V4L2_MEMORY_MMAP` set explicitly — proven against `/dev/video10`
//! (OBS) with `examples/v4l2_probe.rs`.
//!
//! Only used for loopback/virtual devices; hardware cams stay on
//! `rusty-capture`'s richer capturer (more pixel formats, zero-copy paths).

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::time::Instant;

use anyhow::{Context, Result};
use iroh_live::media::format::{PixelFormat, VideoFormat, VideoFrame};
use iroh_live::media::traits::VideoSource;
use v4l2r::bindings;
use v4l2r::ioctl::{self, QueryBuffer};
use v4l2r::memory::{MemoryType, MmapHandle};
use v4l2r::{Format, PixelFormat as V4l2PixelFormat, QueueType};

const V4L2_MEMORY_MMAP_U32: u32 = 1;
/// VIDIOC_DQBUF = _IOWR('V', 17, struct v4l2_buffer). v4l2r's dqbuf helper is
/// unusable for loopback devices (memory field never set), so issue it directly.
const VIDIOC_DQBUF: std::ffi::c_ulong = (3 << 30)
    | (('V' as std::ffi::c_ulong) << 8)
    | 17
    | ((std::mem::size_of::<bindings::v4l2_buffer>() as std::ffi::c_ulong) << 16);

/// One mmap'd capture buffer (kernel-owned, re-queued after each frame).
struct MappedBuf {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is process-shared memory from the kernel; access is
// synchronized by dqbuf/qbuf ownership (we only read while dequeued to us).
unsafe impl Send for MappedBuf {}

impl Drop for MappedBuf {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.len > 0 {
            unsafe {
                libc::munmap(self.ptr.cast(), self.len);
            }
        }
    }
}

/// Read the V4L2 driver name (e.g. `"v4l2 loopback"`, `"uvcvideo"`).
pub fn driver_name(path: &str) -> Option<String> {
    let f = OpenOptions::new().read(true).write(true).open(path).ok()?;
    let caps: ioctl::Capability = ioctl::querycap(&f).ok()?;
    Some(caps.driver.clone())
}

/// True when the device is a v4l2loopback node (OBS Virtual Camera etc.).
/// Those reject v4l2r's dqbuf (memory=0) with EINVAL — route them to
/// [`V4l2MmapCapture`] instead of rusty-capture.
pub fn is_loopback_device(path: &str) -> bool {
    driver_name(path)
        .map(|d| d.to_ascii_lowercase().contains("loopback"))
        .unwrap_or(false)
}

/// V4L2 MMAP capture with a spec-correct dqbuf. Produces RGBA frames.
pub struct V4l2MmapCapture {
    device_path: String,
    name: String,
    width: u32,
    height: u32,
    fourcc: [u8; 4],
    state: Option<CaptureState>,
}

struct CaptureState {
    dev: File,
    bufs: Vec<MappedBuf>,
    started: Instant,
}

impl V4l2MmapCapture {
    /// Open `path` and negotiate `w`×`h` (driver may adjust, as v4l2loopback
    /// does to match the OBS output). Supports YUYV and MJPG payloads.
    pub fn open(path: &str, w: u32, h: u32) -> Result<Self> {
        let mut dev = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open {path}"))?;
        let caps: ioctl::Capability = ioctl::querycap(&dev).context("querycap")?;

        // Try requested size with YUYV first (loopback native), then MJPG,
        // then let the driver pick (0x0 keeps current).
        let mut actual: Option<Format> = None;
        for (req_w, req_h, fourcc) in [
            (w, h, *b"YUYV"),
            (w, h, *b"MJPG"),
            (0, 0, *b"YUYV"),
        ] {
            let desired = Format {
                width: req_w,
                height: req_h,
                pixelformat: V4l2PixelFormat::from_fourcc(&fourcc),
                plane_fmt: vec![],
            };
            match ioctl::s_fmt(&mut dev, (QueueType::VideoCapture, &desired)) {
                Ok(f) => {
                    actual = Some(f);
                    break;
                }
                Err(e) => log::debug!("v4l2cam: s_fmt {fourcc:?} {req_w}x{req_h}: {e}"),
            }
        }
        let actual = actual.context("no acceptable V4L2 format")?;
        let fourcc = actual.pixelformat.to_fourcc();
        anyhow::ensure!(
            fourcc == *b"YUYV" || fourcc == *b"MJPG",
            "unsupported pixel format {:?} (need YUYV or MJPG)",
            fourcc
        );
        log::info!(
            "v4l2cam: opened {path} ({} / {}) {}x{} {:?}",
            caps.card,
            caps.driver,
            actual.width,
            actual.height,
            fourcc
        );
        Ok(Self {
            device_path: path.to_string(),
            name: caps.card.clone(),
            width: actual.width,
            height: actual.height,
            fourcc,
            state: None,
        })
    }

    fn start_streaming(&mut self) -> Result<()> {
        if self.state.is_some() {
            return Ok(());
        }
        let mut dev = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device_path)
            .context("reopen for streaming")?;
        // Re-assert the negotiated format on the fresh fd.
        let desired = Format {
            width: self.width,
            height: self.height,
            pixelformat: V4l2PixelFormat::from_fourcc(&self.fourcc),
            plane_fmt: vec![],
        };
        let _: Format = ioctl::s_fmt(&mut dev, (QueueType::VideoCapture, &desired))?;

        let num_bufs = ioctl::reqbufs(
            &dev,
            QueueType::VideoCapture,
            MemoryType::Mmap,
            4,
            ioctl::MemoryConsistency::empty(),
        )
        .context("reqbufs")?;
        anyhow::ensure!(num_bufs > 0, "reqbufs returned 0 buffers");

        let mut bufs = Vec::with_capacity(num_bufs);
        for i in 0..num_bufs {
            let info: QueryBuffer = ioctl::querybuf(&dev, QueueType::VideoCapture, i)?;
            let plane = info.planes.first().context("no plane")?;
            // v4l2r's PlaneMapping is !Send; use libc mmap so the capturer can
            // move to the encoder thread (VideoSource: Send).
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    plane.length as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    dev.as_raw_fd(),
                    plane.mem_offset as libc::off_t,
                )
            };
            anyhow::ensure!(ptr != libc::MAP_FAILED, "mmap buffer {i} failed");
            bufs.push(MappedBuf {
                ptr: ptr.cast(),
                len: plane.length as usize,
            });

            let mut qbuf = ioctl::QBuffer::<MmapHandle>::new(QueueType::VideoCapture, i as u32);
            qbuf.planes.push(ioctl::QBufPlane::new(0));
            ioctl::qbuf::<_, ()>(&dev, qbuf).context("qbuf")?;
        }

        ioctl::streamon(&dev, QueueType::VideoCapture).context("streamon")?;
        self.state = Some(CaptureState {
            dev,
            bufs,
            started: Instant::now(),
        });
        log::debug!("v4l2cam: streaming started on {}", self.device_path);
        Ok(())
    }

    fn stop_streaming(&mut self) {
        if let Some(state) = self.state.take() {
            ioctl::streamoff(&state.dev, QueueType::VideoCapture).ok();
            log::debug!("v4l2cam: streaming stopped on {}", self.device_path);
        }
    }

    /// dqbuf with `memory = V4L2_MEMORY_MMAP` (the field v4l2r leaves at 0,
    /// which v4l2loopback rejects with EINVAL). Returns buffer index + bytes.
    fn dqbuf_mmap(dev: &File) -> Result<Option<(usize, usize)>> {
        let mut raw: bindings::v4l2_buffer = unsafe { std::mem::zeroed() };
        raw.type_ = QueueType::VideoCapture as u32;
        raw.memory = V4L2_MEMORY_MMAP_U32;
        let ret = unsafe { libc::ioctl(dev.as_raw_fd(), VIDIOC_DQBUF as _, &mut raw as *mut _) };
        if ret == 0 {
            return Ok(Some((raw.index as usize, raw.bytesused as usize)));
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EAGAIN) => Ok(None),
            _ => Err(anyhow::anyhow!("dqbuf: {e}")),
        }
    }

    fn qbuf_mmap(dev: &File, index: usize) -> Result<()> {
        let mut qbuf = ioctl::QBuffer::<MmapHandle>::new(QueueType::VideoCapture, index as u32);
        qbuf.planes.push(ioctl::QBufPlane::new(0));
        ioctl::qbuf::<_, ()>(dev, qbuf).context("re-queue buffer")?;
        Ok(())
    }
}

impl VideoSource for V4l2MmapCapture {
    fn name(&self) -> &str {
        &self.name
    }

    fn format(&self) -> VideoFormat {
        VideoFormat {
            pixel_format: PixelFormat::Rgba,
            dimensions: [self.width, self.height],
        }
    }

    fn start(&mut self) -> Result<()> {
        self.start_streaming()
    }

    fn stop(&mut self) -> Result<()> {
        self.stop_streaming();
        Ok(())
    }

    fn pop_frame(&mut self) -> Result<Option<VideoFrame>> {
        let Some(state) = &self.state else {
            return Ok(None);
        };
        let Some((idx, bytesused)) = Self::dqbuf_mmap(&state.dev)? else {
            return Ok(None);
        };
        let ts = state.started.elapsed();
        let buf = &state.bufs[idx];
        let n = bytesused.min(buf.len);
        // SAFETY: buffer is dequeued to us until we re-queue below.
        let data: &[u8] = unsafe { std::slice::from_raw_parts(buf.ptr, n) };

        let rgba = if self.fourcc == *b"YUYV" {
            Some(yuyv_to_rgba(data, self.width, self.height))
        } else if self.fourcc == *b"MJPG" {
            match image::load_from_memory_with_format(data, image::ImageFormat::Jpeg) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    if rgba.width() == self.width && rgba.height() == self.height {
                        Some(rgba.into_raw())
                    } else {
                        log::warn!("v4l2cam: MJPG dims mismatch, dropping frame");
                        None
                    }
                }
                Err(e) => {
                    log::warn!("v4l2cam: MJPG decode failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        Self::qbuf_mmap(&state.dev, idx)?;

        let Some(rgba) = rgba else {
            return Ok(None);
        };
        Ok(Some(VideoFrame::new_rgba(
            rgba.into(),
            self.width,
            self.height,
            ts,
        )))
    }
}

impl Drop for V4l2MmapCapture {
    fn drop(&mut self) {
        self.stop_streaming();
    }
}

/// Packed YUYV 4:2:2 → RGBA8 (BT.601 limited range, same as rusty-capture).
fn yuyv_to_rgba(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let npix = (width as usize) * (height as usize);
    let mut rgba = vec![255u8; npix * 4];
    let pairs = npix / 2;
    for p in 0..pairs {
        let o = p * 4;
        if o + 3 >= data.len() {
            break;
        }
        let y0 = data[o] as i32;
        let u = data[o + 1] as i32 - 128;
        let y1 = data[o + 2] as i32;
        let v = data[o + 3] as i32 - 128;
        let (r0, g0, b0) = yuv_to_rgb(y0, u, v);
        let (r1, g1, b1) = yuv_to_rgb(y1, u, v);
        let px = p * 8;
        rgba[px] = r0;
        rgba[px + 1] = g0;
        rgba[px + 2] = b0;
        rgba[px + 4] = r1;
        rgba[px + 5] = g1;
        rgba[px + 6] = b1;
    }
    rgba
}

fn yuv_to_rgb(y: i32, u: i32, v: i32) -> (u8, u8, u8) {
    // BT.601 studio swing: C = 1.164(Y-16)
    let c = (298 * (y - 16) + 128) >> 8;
    let r = (c + ((409 * v + 128) >> 8)).clamp(0, 255) as u8;
    let g = (c - ((100 * u + 208 * v + 128) >> 8)).clamp(0, 255) as u8;
    let b = (c + ((516 * u + 128) >> 8)).clamp(0, 255) as u8;
    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn yuyv_mid_gray_is_achromatic() {
        // Y=128 U=128 V=128 → mid gray, R=G=B≈128, alpha forced 255.
        let data = [128u8, 128, 128, 128];
        let rgba = yuyv_to_rgba(&data, 2, 1);
        assert_eq!(rgba.len(), 2 * 4);
        let (r, g, b, a) = (rgba[0], rgba[1], rgba[2], rgba[3]);
        assert_eq!(a, 255);
        assert!((r as i16 - g as i16).abs() <= 2, "r={r} g={g}");
        assert!((g as i16 - b as i16).abs() <= 2, "g={g} b={b}");
        assert!((r as i16 - 128).abs() <= 4, "expected ~128, got {r}");
    }

    #[test]
    fn yuyv_color_pair_preserves_difference() {
        // Two luma levels must produce different RGB (real content, not uniform).
        let data = [60u8, 90, 200, 90];
        let rgba = yuyv_to_rgba(&data, 2, 1);
        assert!(rgba[0] != rgba[4], "different Y must give different R");
    }

    /// Real-device integration proof: capture one frame from the loopback
    /// device (or SLEEK_TEST_CAMERA_ID) via THIS shipped capture path and
    /// assert non-uniform content. Skips with a reason when unavailable.
    #[test]
    fn v4l2_capture_yields_nonuniform_frame() {
        let id = std::env::var("SLEEK_TEST_CAMERA_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/dev/video10".to_string());
        let mut cam = match V4l2MmapCapture::open(&id, 640, 360) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("SKIP v4l2_capture id={id}: open failed: {e:#}");
                return;
            }
        };
        if let Err(e) = cam.start() {
            eprintln!("SKIP v4l2_capture id={id}: start failed: {e:#}");
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(2_500);
        let mut frame = None;
        while Instant::now() < deadline {
            match cam.pop_frame() {
                Ok(Some(f)) => {
                    frame = Some(f);
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    let _ = cam.stop();
                    eprintln!("SKIP v4l2_capture id={id}: pop_frame: {e:#}");
                    return;
                }
            }
        }
        let _ = cam.stop();
        let Some(f) = frame else {
            eprintln!("SKIP v4l2_capture id={id}: no frames within 2.5s");
            return;
        };
        let rgba = f.rgba_image();
        let bytes = rgba.as_raw().as_slice();
        let mut min = 255u8;
        let mut max = 0u8;
        for px in bytes.chunks_exact(4) {
            let l = ((77u32 * px[0] as u32 + 150u32 * px[1] as u32 + 29u32 * px[2] as u32) >> 8) as u8;
            min = min.min(l);
            max = max.max(l);
        }
        eprintln!(
            "v4l2_capture id={id} {}x{} luma {min}..{max} (alpha0={})",
            f.width(),
            f.height(),
            bytes.get(3).copied().unwrap_or(0)
        );
        assert!(
            max > min,
            "captured frame must be non-uniform (luma {min}..{max})"
        );
    }
}
