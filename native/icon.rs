//! Procedural WheelFlow icon: a mouse with its wheel, anti-aliased at any size.
//! Shared by the build script (executable icon) and the tray (enabled/paused).

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Enabled,
    Paused,
}

// Colors as (red, green, blue).
const ENABLED_BODY: (f64, f64, f64) = (37.0, 99.0, 235.0);
const ENABLED_EDGE: (f64, f64, f64) = (30.0, 58.0, 138.0);
const PAUSED_BODY: (f64, f64, f64) = (120.0, 125.0, 135.0);
const PAUSED_EDGE: (f64, f64, f64) = (75.0, 80.0, 90.0);
const WHEEL: (f64, f64, f64) = (255.0, 255.0, 255.0);

/// Signed distance from a point to a rounded rectangle centred at (cx, cy).
fn rounded_box(x: f64, y: f64, cx: f64, cy: f64, half_w: f64, half_h: f64, radius: f64) -> f64 {
    let qx = (x - cx).abs() - (half_w - radius);
    let qy = (y - cy).abs() - (half_h - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// Pixel coverage of a shape from its signed distance in pixels.
fn coverage(distance_px: f64) -> f64 {
    (0.5 - distance_px).clamp(0.0, 1.0)
}

/// Renders a square icon as straight-alpha BGRA bytes, top row first.
pub fn render(size: u32, look: Look) -> Vec<u8> {
    let (body, edge) = match look {
        Look::Enabled => (ENABLED_BODY, ENABLED_EDGE),
        Look::Paused => (PAUSED_BODY, PAUSED_EDGE),
    };
    let scale = size as f64;
    // Outline thickness: one pixel at small sizes, proportionally more when large.
    let edge_px = (scale / 24.0).max(1.0);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for row in 0..size {
        for column in 0..size {
            // Normalized pixel centre.
            let x = (column as f64 + 0.5) / scale;
            let y = (row as f64 + 0.5) / scale;
            let outer = rounded_box(x, y, 0.5, 0.5, 0.34, 0.46, 0.30) * scale;
            let wheel = rounded_box(x, y, 0.5, 0.30, 0.075, 0.14, 0.075) * scale;
            let shape = coverage(outer);
            let fill = coverage(outer + edge_px);
            let mark = coverage(wheel);
            // Layer: edge colour, body fill inside it, wheel on top.
            let mix = |a: f64, b: f64, t: f64| a + (b - a) * t;
            let mut color = (
                mix(edge.0, body.0, fill),
                mix(edge.1, body.1, fill),
                mix(edge.2, body.2, fill),
            );
            color = (
                mix(color.0, WHEEL.0, mark),
                mix(color.1, WHEEL.1, mark),
                mix(color.2, WHEEL.2, mark),
            );
            let alpha = shape.max(mark);
            pixels.extend_from_slice(&[
                color.2.round() as u8,
                color.1.round() as u8,
                color.0.round() as u8,
                (alpha * 255.0).round() as u8,
            ]);
        }
    }
    pixels
}

/// A Windows icon image (BITMAPINFOHEADER, bottom-up BGRA, AND mask) as used
/// in .ico files and RT_ICON resources. Used by the build script.
#[allow(dead_code)]
pub fn icon_image(size: u32, look: Look) -> Vec<u8> {
    let pixels = render(size, look);
    let mask_stride = ((size + 31) / 32 * 4) as usize;
    let mut data = Vec::new();
    let header: [u32; 10] = [
        40,
        size,
        size * 2,
        1 | (32 << 16),
        0,
        size * size * 4 + (mask_stride * size as usize) as u32,
        0,
        0,
        0,
        0,
    ];
    for value in header {
        data.extend_from_slice(&value.to_le_bytes());
    }
    let row_bytes = (size * 4) as usize;
    for row in (0..size as usize).rev() {
        data.extend_from_slice(&pixels[row * row_bytes..(row + 1) * row_bytes]);
    }
    for row in (0..size as usize).rev() {
        let mut mask = vec![0u8; mask_stride];
        for column in 0..size as usize {
            if pixels[row * row_bytes + column * 4 + 3] == 0 {
                mask[column / 8] |= 0x80 >> (column % 8);
            }
        }
        data.extend_from_slice(&mask);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::{icon_image, render, Look};

    #[test]
    fn icons_are_opaque_inside_transparent_outside_and_distinct() {
        for size in [16, 20, 24, 32, 48, 64] {
            let enabled = render(size, Look::Enabled);
            let paused = render(size, Look::Paused);
            assert_eq!(enabled.len(), (size * size * 4) as usize);
            let alpha = |pixels: &[u8], x: u32, y: u32| pixels[((y * size + x) * 4 + 3) as usize];
            assert_eq!(alpha(&enabled, 0, 0), 0, "corner is transparent");
            assert_eq!(
                alpha(&enabled, size / 2, size * 3 / 4),
                255,
                "body is opaque"
            );
            assert_ne!(enabled, paused);
            // The wheel is white against the body.
            let wheel = (((size * 31 / 100) * size + size / 2) * 4) as usize;
            assert!(enabled[wheel] > 200 && enabled[wheel + 1] > 200);
        }
    }

    #[test]
    fn icon_image_has_header_pixels_and_mask() {
        let image = icon_image(16, Look::Enabled);
        assert_eq!(&image[..4], &40u32.to_le_bytes());
        assert_eq!(image.len(), 40 + 16 * 16 * 4 + 4 * 16);
    }
}
