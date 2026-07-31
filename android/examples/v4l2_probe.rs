//! Probe: does v4l2r's dqbuf (memory field left 0) fail on v4l2loopback
//! (OBS Virtual Camera) with EINVAL, while a dqbuf with memory=MMAP succeeds?
//!
//! Run: cargo run --example v4l2_probe -- /dev/video10

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

use v4l2r::bindings;
use v4l2r::ioctl::{self, PlaneMapping, QueryBuffer};
use v4l2r::memory::{MemoryType, MmapHandle};
use v4l2r::{Format, PixelFormat as V4l2PixelFormat, QueueType};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/video10".to_string());
    println!("probing {path}");

    let mut dev = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open");

    // Negotiate YUYV 640x360 (device may adjust).
    let desired = Format {
        width: 640,
        height: 360,
        pixelformat: V4l2PixelFormat::from_fourcc(b"YUYV"),
        plane_fmt: vec![],
    };
    let actual: Format =
        ioctl::s_fmt(&mut dev, (QueueType::VideoCapture, &desired)).expect("s_fmt");
    println!(
        "s_fmt actual: {}x{} {:?}",
        actual.width,
        actual.height,
        actual.pixelformat.to_fourcc()
    );

    let num_bufs: usize = ioctl::reqbufs(
        &dev,
        QueueType::VideoCapture,
        MemoryType::Mmap,
        4,
        ioctl::MemoryConsistency::empty(),
    )
    .expect("reqbufs");
    println!("reqbufs: {num_bufs}");

    let mut mappings: Vec<PlaneMapping> = Vec::new();
    for i in 0..num_bufs {
        let buf_info: QueryBuffer = ioctl::querybuf(&dev, QueueType::VideoCapture, i).expect("querybuf");
        let plane = buf_info.planes.first().expect("plane").clone();
        mappings.push(ioctl::mmap(&dev, plane.mem_offset, plane.length).expect("mmap"));
        let mut qbuf = ioctl::QBuffer::<MmapHandle>::new(QueueType::VideoCapture, i as u32);
        qbuf.planes.push(ioctl::QBufPlane::new(0));
        ioctl::qbuf::<_, ()>(&dev, qbuf).expect("qbuf");
    }
    println!("queued {num_bufs} buffers");

    ioctl::streamon(&dev, QueueType::VideoCapture).expect("streamon");
    println!("streamon ok");

    // 1) v4l2r dqbuf (memory field = 0) — expected to EINVAL on v4l2loopback.
    match ioctl::dqbuf::<QueryBuffer>(&dev, QueueType::VideoCapture) {
        Ok(buf) => println!("v4l2r dqbuf (memory=0): OK index {}", buf.index),
        Err(e) => println!("v4l2r dqbuf (memory=0): FAIL {e:?}"),
    }

    // 2) Manual dqbuf with memory = V4L2_MEMORY_MMAP — expected to work.
    let mut raw: bindings::v4l2_buffer = unsafe { std::mem::zeroed() };
    raw.type_ = QueueType::VideoCapture as u32;
    raw.memory = 1; // V4L2_MEMORY_MMAP
    // VIDIOC_DQBUF = _IOWR('V', 17, v4l2_buffer)
    const VIDIOC_DQBUF: std::ffi::c_ulong = (3 << 30)
        | (('V' as std::ffi::c_ulong) << 8)
        | 17
        | ((std::mem::size_of::<bindings::v4l2_buffer>() as std::ffi::c_ulong) << 16);
    let ret = unsafe {
        libc::ioctl(dev.as_raw_fd(), VIDIOC_DQBUF as _, &mut raw as *mut _)
    };
    if ret == 0 {
        println!(
            "manual dqbuf (memory=MMAP): OK index {} bytesused {}",
            raw.index, raw.bytesused
        );
        let idx = raw.index as usize;
        let len = raw.bytesused as usize;
        let data: &[u8] = &mappings[idx][..len.min(mappings[idx].len())];
        if !data.is_empty() {
            let mut min = 255u8;
            let mut max = 0u8;
            for y in data.iter().step_by(2) {
                min = min.min(*y);
                max = max.max(*y);
            }
            println!("Y luma range: {min}..{max} ({} bytes, range>0 = real content)", data.len());
        }
    } else {
        let e = std::io::Error::last_os_error();
        println!("manual dqbuf (memory=MMAP): FAIL {e:?}");
    }

    ioctl::streamoff(&dev, QueueType::VideoCapture).ok();
    println!("done");
}
