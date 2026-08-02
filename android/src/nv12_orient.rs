//! Rotate NV12 frames so Camera2 sensor buffers appear upright.
//!
//! Phone sensors are usually mounted at 90°/270°. ImageReader delivers
//! buffers in sensor coordinates; without a CW rotate by
//! [`CameraCharacteristics.SENSOR_ORIENTATION`] ± display rotation, portrait
//! video looks sideways on the wire and in local preview.

/// Orient an NV12 frame by rotating `rotation_cw` degrees clockwise (0/90/180/270).
///
/// Stride padding is stripped. For 90°/270°, width and height are swapped.
/// Odd dimensions are rejected (YUV 4:2:0 requires even chroma grid).
pub fn orient_nv12(
    y_data: &[u8],
    uv_data: &[u8],
    width: u32,
    height: u32,
    y_stride: u32,
    uv_stride: u32,
    rotation_cw: u32,
) -> Option<(Vec<u8>, Vec<u8>, u32, u32)> {
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return None;
    }
    if y_stride < width || uv_stride < width {
        return None;
    }
    let (y, uv) = pack_nv12(y_data, uv_data, width, height, y_stride, uv_stride)?;
    let rot = normalize_rotation(rotation_cw);
    match rot {
        0 => Some((y, uv, width, height)),
        90 => Some(rotate_nv12_90_cw(&y, &uv, width, height)),
        180 => Some(rotate_nv12_180(&y, &uv, width, height)),
        270 => Some(rotate_nv12_270_cw(&y, &uv, width, height)),
        _ => Some((y, uv, width, height)),
    }
}

fn normalize_rotation(degrees: u32) -> u32 {
    let d = degrees % 360;
    // Snap near-miss values from noisy sensors to the nearest cardinal.
    match d {
        0..=44 | 316..=359 => 0,
        45..=134 => 90,
        135..=224 => 180,
        225..=315 => 270,
        _ => 0,
    }
}

fn pack_nv12(
    y_data: &[u8],
    uv_data: &[u8],
    width: u32,
    height: u32,
    y_stride: u32,
    uv_stride: u32,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let w = width as usize;
    let h = height as usize;
    let ys = y_stride as usize;
    let uvs = uv_stride as usize;
    let y_need = ys.checked_mul(h)?;
    let uv_need = uvs.checked_mul(h / 2)?;
    if y_data.len() < y_need || uv_data.len() < uv_need {
        return None;
    }
    let mut y_out = vec![0u8; w * h];
    for row in 0..h {
        let src = row * ys;
        let dst = row * w;
        y_out[dst..dst + w].copy_from_slice(&y_data[src..src + w]);
    }
    let mut uv_out = vec![0u8; w * (h / 2)];
    for row in 0..(h / 2) {
        let src = row * uvs;
        let dst = row * w;
        uv_out[dst..dst + w].copy_from_slice(&uv_data[src..src + w]);
    }
    Some((y_out, uv_out))
}

/// 90° CW: dst(dx, dy) = src(dy, H-1-dx); output is H×W.
fn rotate_nv12_90_cw(y: &[u8], uv: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let w = width as usize;
    let h = height as usize;
    let out_w = h;
    let out_h = w;
    let mut y_out = vec![0u8; out_w * out_h];
    for dy in 0..out_h {
        for dx in 0..out_w {
            let sx = dy;
            let sy = h - 1 - dx;
            y_out[dy * out_w + dx] = y[sy * w + sx];
        }
    }
    let cw = w / 2;
    let ch = h / 2;
    let out_cw = ch;
    let out_ch = cw;
    let mut uv_out = vec![0u8; out_w * (out_h / 2)];
    for dy in 0..out_ch {
        for dx in 0..out_cw {
            let sx = dy;
            let sy = ch - 1 - dx;
            let src = (sy * cw + sx) * 2;
            let dst = (dy * out_cw + dx) * 2;
            uv_out[dst] = uv[src];
            uv_out[dst + 1] = uv[src + 1];
        }
    }
    (y_out, uv_out, out_w as u32, out_h as u32)
}

/// 270° CW (= 90° CCW): dst(dx, dy) = src(W-1-dy, dx); output is H×W.
fn rotate_nv12_270_cw(
    y: &[u8],
    uv: &[u8],
    width: u32,
    height: u32,
) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let w = width as usize;
    let h = height as usize;
    let out_w = h;
    let out_h = w;
    let mut y_out = vec![0u8; out_w * out_h];
    for dy in 0..out_h {
        for dx in 0..out_w {
            let sx = w - 1 - dy;
            let sy = dx;
            y_out[dy * out_w + dx] = y[sy * w + sx];
        }
    }
    let cw = w / 2;
    let ch = h / 2;
    let out_cw = ch;
    let out_ch = cw;
    let mut uv_out = vec![0u8; out_w * (out_h / 2)];
    for dy in 0..out_ch {
        for dx in 0..out_cw {
            let sx = cw - 1 - dy;
            let sy = dx;
            let src = (sy * cw + sx) * 2;
            let dst = (dy * out_cw + dx) * 2;
            uv_out[dst] = uv[src];
            uv_out[dst + 1] = uv[src + 1];
        }
    }
    (y_out, uv_out, out_w as u32, out_h as u32)
}

fn rotate_nv12_180(y: &[u8], uv: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let w = width as usize;
    let h = height as usize;
    let mut y_out = vec![0u8; w * h];
    for dy in 0..h {
        for dx in 0..w {
            let sx = w - 1 - dx;
            let sy = h - 1 - dy;
            y_out[dy * w + dx] = y[sy * w + sx];
        }
    }
    let cw = w / 2;
    let ch = h / 2;
    let mut uv_out = vec![0u8; w * (h / 2)];
    for dy in 0..ch {
        for dx in 0..cw {
            let sx = cw - 1 - dx;
            let sy = ch - 1 - dy;
            let src = (sy * cw + sx) * 2;
            let dst = (dy * cw + dx) * 2;
            uv_out[dst] = uv[src];
            uv_out[dst + 1] = uv[src + 1];
        }
    }
    (y_out, uv_out, width, height)
}

/// Camera2 JPEG / buffer orientation: degrees CW to apply so the frame is
/// upright for the current display rotation.
pub fn camera2_rotation_degrees(
    sensor_orientation: u32,
    display_degrees: u32,
    front_facing: bool,
) -> u32 {
    let sensor = sensor_orientation % 360;
    let display = display_degrees % 360;
    if front_facing {
        (sensor + display) % 360
    } else {
        (sensor + 360 - display) % 360
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_nv12(w: u32, h: u32, y: u8, u: u8, v: u8) -> (Vec<u8>, Vec<u8>) {
        let y_plane = vec![y; (w * h) as usize];
        let mut uv = vec![0u8; (w * (h / 2)) as usize];
        for i in 0..(uv.len() / 2) {
            uv[i * 2] = u;
            uv[i * 2 + 1] = v;
        }
        (y_plane, uv)
    }

    /// Marker at (sx,sy) on a black Y plane — used to verify rotate mapping.
    fn marker_nv12(w: u32, h: u32, sx: u32, sy: u32) -> (Vec<u8>, Vec<u8>) {
        let (mut y, uv) = solid_nv12(w, h, 0, 128, 128);
        y[(sy * w + sx) as usize] = 255;
        (y, uv)
    }

    #[test]
    fn identity_keeps_dims_and_marker() {
        let (y, uv) = marker_nv12(4, 4, 1, 0);
        let (oy, _ouv, ow, oh) = orient_nv12(&y, &uv, 4, 4, 4, 4, 0).unwrap();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!(oy[1], 255);
    }

    #[test]
    fn rotate_90_cw_moves_top_left_edge_marker() {
        // Marker at top row, x=1 → after 90° CW sits at right column, y=1.
        // dst(dx,dy)=src(dy,H-1-dx) ⇒ src(1,0) → dx=H-1-0=3, dy=1.
        let (y, uv) = marker_nv12(4, 4, 1, 0);
        let (oy, _, ow, oh) = orient_nv12(&y, &uv, 4, 4, 4, 4, 90).unwrap();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!(oy[1 * 4 + 3], 255);
    }

    #[test]
    fn rotate_90_swaps_rect_dims() {
        let (y, uv) = solid_nv12(8, 4, 16, 80, 160);
        let (oy, ouv, ow, oh) = orient_nv12(&y, &uv, 8, 4, 8, 8, 90).unwrap();
        assert_eq!((ow, oh), (4, 8));
        assert_eq!(oy.len(), 4 * 8);
        assert_eq!(ouv.len(), 4 * 4);
    }

    #[test]
    fn rotate_180_moves_marker_to_opposite_corner() {
        let (y, uv) = marker_nv12(4, 4, 0, 0);
        let (oy, _, ow, oh) = orient_nv12(&y, &uv, 4, 4, 4, 4, 180).unwrap();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!(oy[3 * 4 + 3], 255);
    }

    #[test]
    fn rotate_270_cw_moves_marker() {
        // src(1,0) → 270 CW: sx=W-1-dy, sy=dx ⇒ dy=W-1-1=2, dx=0 → (0,2)
        let (y, uv) = marker_nv12(4, 4, 1, 0);
        let (oy, _, ow, oh) = orient_nv12(&y, &uv, 4, 4, 4, 4, 270).unwrap();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!(oy[2 * 4 + 0], 255);
    }

    #[test]
    fn strips_y_stride_padding() {
        let w = 4u32;
        let h = 4u32;
        let y_stride = 8u32;
        let mut y = vec![0u8; (y_stride * h) as usize];
        // Put marker at (1,0) in logical coords (byte 1 of row 0).
        y[1] = 255;
        let uv = vec![128u8; (w * (h / 2)) as usize];
        let (oy, _, ow, oh) = orient_nv12(&y, &uv, w, h, y_stride, w, 0).unwrap();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!(oy[1], 255);
        assert_eq!(oy.len(), 16);
    }

    #[test]
    fn camera2_back_portrait_sensor_90() {
        // Typical back camera, phone held in natural portrait (display 0°).
        assert_eq!(camera2_rotation_degrees(90, 0, false), 90);
    }

    #[test]
    fn camera2_front_portrait_sensor_270() {
        assert_eq!(camera2_rotation_degrees(270, 0, true), 270);
    }

    #[test]
    fn camera2_back_landscape_display_90() {
        // Rotated to landscape: sensor 90 − display 90 = 0 (already upright).
        assert_eq!(camera2_rotation_degrees(90, 90, false), 0);
    }

    #[test]
    fn rejects_odd_dimensions() {
        let (y, uv) = solid_nv12(4, 4, 0, 128, 128);
        assert!(orient_nv12(&y, &uv, 3, 4, 3, 3, 0).is_none());
    }
}
