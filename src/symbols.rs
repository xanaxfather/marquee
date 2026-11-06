//! macOS menu icons: SF Symbols rendered at runtime, so they always match
//! the system's own glyphs and follow light/dark mode (template images).

use objc2::AnyThread;
use objc2_app_kit::{
    NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSFontWeightRegular, NSGraphicsContext, NSImage,
    NSImageSymbolConfiguration,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tray_icon::menu::Icon;

/// 18 pt menu icon slot at 2× → 36 px; muda draws it at 18 pt.
const PX: isize = 36;
/// Glyph size, matching the symbols in system menus.
const POINT_SIZE: f64 = 26.0;

/// Render an SF Symbol as a template (alpha-only) icon. Must run on the main thread.
pub fn symbol(name: &str) -> Option<Icon> {
    let rgba = symbol_rgba(name)?;
    Icon::from_rgba(rgba, PX as u32, PX as u32).ok()
}

fn symbol_rgba(name: &str) -> Option<Vec<u8>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
    let config =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(POINT_SIZE, unsafe { NSFontWeightRegular });
    let img = img.imageWithSymbolConfiguration(&config)?;

    // SAFETY: plain RGBA8 bitmap; AppKit allocates the buffer (planes = null).
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            PX,
            PX,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    let s = img.size();
    // Fit (wide symbols) and centre.
    let scale = (PX as f64 / s.width.max(s.height)).min(1.0);
    let (w, h) = (s.width * scale, s.height * scale);
    let rect = NSRect::new(NSPoint::new((PX as f64 - w) / 2.0, (PX as f64 - h) / 2.0), NSSize::new(w, h));
    img.drawInRect_fromRect_operation_fraction(rect, NSRect::ZERO, NSCompositingOperation::SourceOver, 1.0);
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let data = rep.bitmapData();
    if data.is_null() {
        return None;
    }
    let stride = rep.bytesPerRow() as usize;
    let mut rgba = vec![0u8; (PX * PX * 4) as usize];
    for y in 0..PX as usize {
        for x in 0..PX as usize {
            // SAFETY: within the rep's buffer (stride × PX bytes).
            let a = unsafe { *data.add(y * stride + x * 4 + 3) };
            rgba[(y * PX as usize + x) * 4 + 3] = a; // template: only alpha matters
        }
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const MENU_SYMBOLS: &[&str] = &[
        "play.circle",
        "pause.circle",
        "stop.circle",
        "eye.slash",
        "wrench.and.screwdriver",
        "exclamationmark.triangle",
        "photo.on.rectangle.angled",
        "arrow.triangle.2.circlepath",
        "arrow.clockwise",
        "slider.horizontal.3",
        "doc.text.magnifyingglass",
        "trash",
        "power",
    ];

    #[test]
    fn symbols_render() {
        // SYMBOL_DUMP=<file.pgm> writes a contact sheet of the alpha masks.
        let mut sheet = Vec::new();
        for n in MENU_SYMBOLS {
            let rgba = symbol_rgba(n).unwrap_or_else(|| panic!("SF Symbol {n} missing"));
            let alpha: Vec<u8> = rgba.chunks(4).map(|p| p[3]).collect();
            let cover = alpha.iter().filter(|a| **a > 128).count() as f64 / alpha.len() as f64;
            assert!((0.04..0.7).contains(&cover), "{n}: coverage {cover:.2}");
            sheet.push(alpha);
        }
        if let Some(path) = std::env::var_os("SYMBOL_DUMP") {
            let (w, h) = (PX as usize * sheet.len(), PX as usize);
            let mut pgm = format!("P5 {w} {h} 255\n").into_bytes();
            for y in 0..h {
                for a in &sheet {
                    pgm.extend_from_slice(&a[y * PX as usize..(y + 1) * PX as usize]);
                }
            }
            std::fs::write(path, pgm).unwrap();
        }
    }
}
