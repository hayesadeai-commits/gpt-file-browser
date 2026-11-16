//! Original application icon rendered without external image dependencies.
//!
//! The executable resource and native window use this same rasterizer. The small
//! inline SVG in ui.html mirrors the mark for the web-based application header.

// Original GPT File Browser character, shared by the window and executable resource.
/// Test a point against a rounded rectangle in the 64-unit design coordinate system.
fn rounded(x: f32, y: f32, rect: [f32; 4], radius: f32) -> bool {
    let [left, top, right, bottom] = rect;
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}
/// Sample the full-canvas folder character. Shapes reach within one design unit
/// of the edges, keeping the face readable at taskbar size without an inset badge.
fn pixel(x: f32, y: f32) -> [u8; 4] {
    let mut c = [0; 4];
    // Folder tab and back: the silhouette itself fills the icon, not a dark tile.
    if rounded(x, y, [1., 1., 32., 28.], 7.) || rounded(x, y, [1., 10., 63., 59.], 8.) {
        c = [30, 109, 91, 255];
    }
    if rounded(x, y, [2., 2., 31., 27.], 6.) || rounded(x, y, [2., 11., 62., 58.], 7.) {
        c = [89, 194, 154, 255];
    }
    // A cream document peeks above the front pocket.
    if rounded(x, y, [12., 7., 54., 36.], 4.) {
        c = [248, 250, 230, 255];
    }
    if rounded(x, y, [18., 12., 44., 14.], 1.) {
        c = [154, 192, 165, 255];
    }
    // Bold border, mint face, and a gentle vertical color change.
    if rounded(x, y, [1., 18., 63., 63.], 9.) {
        c = [27, 99, 83, 255];
    }
    if rounded(x, y, [2., 19., 62., 62.], 8.) {
        let t = ((y - 19.) / 43.).clamp(0., 1.);
        c = [
            (154. - 44. * t) as u8,
            (242. - 24. * t) as u8,
            (205. - 21. * t) as u8,
            255,
        ];
    }
    if rounded(x, y, [8., 21., 55., 23.], 1.) {
        c = [202, 255, 228, 255];
    }
    // Wide-set eyes, tiny highlights, and rosy cheeks make the folder friendly.
    for cx in [22., 42.] {
        if ((x - cx) / 3.).powi(2) + ((y - 37.) / 3.8).powi(2) <= 1. {
            c = [22, 66, 59, 255];
        }
        if (x - (cx - 0.8)).powi(2) + (y - 35.8).powi(2) <= 0.9 {
            c = [250, 255, 245, 255];
        }
    }
    for cx in [13., 51.] {
        if ((x - cx) / 4.5).powi(2) + ((y - 44.) / 2.5).powi(2) <= 1. {
            c = [245, 156, 158, 255];
        }
    }
    let smile = ((x - 32.).powi(2) + (y - 40.).powi(2)).sqrt();
    if y > 42. && (5.3..7.3).contains(&smile) {
        c = [22, 66, 59, 255];
    }
    // A small golden sparkle adds character without crowding the face.
    if (x - 57.).abs() / 5. + (y - 7.).abs() / 6. <= 1. {
        c = [255, 216, 116, 255];
    }
    c
}
/// Rasterize at the requested square size with 4x4 supersampling per pixel.
/// Average premultiplied color before dividing by alpha to avoid dark transparent edges.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut sums = [0u32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let c = pixel(
                        (x as f32 + (sx as f32 + 0.5) / 4.) * 64. / size as f32,
                        (y as f32 + (sy as f32 + 0.5) / 4.) * 64. / size as f32,
                    );
                    for i in 0..3 {
                        sums[i] += c[i] as u32 * c[3] as u32;
                    }
                    sums[3] += c[3] as u32;
                }
            }
            for sum in &sums[..3] {
                out.push(sum.checked_div(sums[3]).unwrap_or(0) as u8);
            }
            out.push((sums[3] / 16) as u8);
        }
    }
    out
}
#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_transparency_and_expected_dimensions() {
        let pixels = super::rgba(32);
        assert_eq!(pixels.len(), 32 * 32 * 4);
        assert_eq!(pixels[3], 0);
        assert_eq!(pixels[(16 * 32 + 16) * 4 + 3], 255);
        // Opaque bounds must occupy nearly the entire 32px canvas, not an inset glyph.
        let points: Vec<_> = (0..32)
            .flat_map(|y| (0..32).map(move |x| (x, y)))
            .filter(|(x, y)| pixels[(y * 32 + x) * 4 + 3] > 128)
            .collect();
        assert!(
            points.iter().map(|p| p.0).max().unwrap() - points.iter().map(|p| p.0).min().unwrap()
                >= 29
        );
        assert!(
            points.iter().map(|p| p.1).max().unwrap() - points.iter().map(|p| p.1).min().unwrap()
                >= 29
        );
    }
}
