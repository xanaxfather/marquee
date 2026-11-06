//! Small icons drawn at runtime (no image files to ship): the tray icon in
//! the same shape as the app icon, and coloured status dots for the menu.

const SS: usize = 4; // 4×4 supersampling for smooth edges

/// Rasterize `inside(x, y)` (in pixel units) into an RGBA buffer of one colour.
fn raster(size: usize, rgb: (u8, u8, u8), inside: impl Fn(f32, f32) -> bool) -> Vec<u8> {
    let mut rgba = vec![0u8; size * size * 4];
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if inside(x, y) {
                        hits += 1;
                    }
                }
            }
            let i = (py * size + px) * 4;
            rgba[i] = rgb.0;
            rgba[i + 1] = rgb.1;
            rgba[i + 2] = rgb.2;
            rgba[i + 3] = (hits * 255 / (SS * SS)) as u8;
        }
    }
    rgba
}

fn in_rounded(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    x >= x0 && x <= x1 && y >= y0 && y <= y1 && (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// Play triangle pointing right, optically centred on (cx, cy), `h` tall.
fn in_play(x: f32, y: f32, cx: f32, cy: f32, h: f32) -> bool {
    let w = h * 0.88;
    let x0 = cx - w * 0.42;
    let (ax, ay, bx, by, qx, qy) = (x0, cy - h / 2.0, x0, cy + h / 2.0, x0 + w, cy);
    let s = |px: f32, py: f32, ux: f32, uy: f32, vx: f32, vy: f32| (px - vx) * (uy - vy) - (ux - vx) * (py - vy);
    let d1 = s(x, y, ax, ay, bx, by);
    let d2 = s(x, y, bx, by, qx, qy);
    let d3 = s(x, y, qx, qy, ax, ay);
    !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
}

/// macOS menu bar: 18 pt template (36 px @2x) — screen outline + play.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn tray_template() -> (Vec<u8>, u32) {
    const S: usize = 36;
    let rgba = raster(S, (0, 0, 0), |x, y| {
        let outer = in_rounded(x, y, 2.0, 7.0, 34.0, 29.0, 6.0);
        let inner = in_rounded(x, y, 5.5, 10.5, 30.5, 25.5, 3.0);
        (outer && !inner) || in_play(x, y, 18.0, 18.0, 9.5)
    });
    (rgba, S as u32)
}

/// Windows tray: full-colour tile (tray icons there are not monochrome).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn tray_color() -> (Vec<u8>, u32) {
    const S: usize = 32;
    let tile = raster(S, (0x58, 0x65, 0xf2), |x, y| in_rounded(x, y, 1.0, 1.0, 31.0, 31.0, 7.0));
    let glyph = raster(S, (255, 255, 255), |x, y| in_play(x, y, 16.0, 16.0, 15.0));
    // Composite white glyph over the tile.
    let mut out = tile;
    for i in (0..out.len()).step_by(4) {
        let a = glyph[i + 3] as u32;
        for c in 0..3 {
            out[i + c] = ((out[i + c] as u32 * (255 - a) + 255 * a) / 255) as u8;
        }
    }
    (out, S as u32)
}

/// A filled status dot for menu items (keeps its colour).
pub fn dot(rgb: (u8, u8, u8)) -> (Vec<u8>, u32) {
    const S: usize = 36;
    let c = S as f32 / 2.0;
    let rgba = raster(S, rgb, |x, y| (x - c).powi(2) + (y - c).powi(2) <= 7.5 * 7.5);
    (rgba, S as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_have_sensible_coverage() {
        for (rgba, size) in [tray_template(), tray_color(), dot((1, 2, 3))] {
            let px = (size * size) as usize;
            assert_eq!(rgba.len(), px * 4);
            let solid = rgba.chunks(4).filter(|p| p[3] > 128).count() as f64 / px as f64;
            assert!((0.1..0.95).contains(&solid), "coverage {solid}");
        }
    }
}
