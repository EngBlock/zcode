//! Thumbnails of a catalog's images, decoded off the main thread at most a
//! few at a time, scaled down to `MAX_PIXELS` and kept in a cache shared by
//! every window (least recently shown dropped past `BUDGET` bytes). Only
//! what's on screen is asked for, so a catalog of thousands of images costs
//! what one screenful does.
//!
//! PNG, JPEG, GIF, TIFF, BMP, WebP and ICO decode with the `image` crate
//! (GPUI's own), SVG with `resvg` (also GPUI's), PDF with Core Graphics,
//! and anything else macOS reads (HEIC, JPEG 2000…) through `sips`.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::*;

/// The longest side of a thumbnail, in pixels (wells are ~96 points, at 2x).
pub const MAX_PIXELS: u32 = 192;
/// What the cache keeps before dropping the least recently shown.
const BUDGET: usize = 64 * 1024 * 1024;
/// Decodes at once.
const CONCURRENCY: usize = 4;

/// A file's modification time and length: a changed file gets a new
/// thumbnail.
pub type Stamp = (Option<SystemTime>, u64);

/// A decoded thumbnail: straight-alpha RGBA rows, its size, and the size
/// of the image it was made from (in pixels; a PDF's or SVG's in points).
#[derive(Clone, Debug)]
pub struct Decoded {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source: (u32, u32),
}

pub struct Thumbnail {
    pub image: Arc<RenderImage>,
    /// The image's own size.
    pub source: (u32, u32),
    bytes: usize,
}

#[derive(Clone)]
pub enum State {
    Loading,
    Ready(Arc<Thumbnail>),
    Failed(String),
}

impl State {
    pub fn id(&self) -> &'static str {
        match self {
            State::Loading => "loading",
            State::Ready(_) => "ready",
            State::Failed(_) => "failed",
        }
    }
}

struct Entry {
    stamp: Stamp,
    state: State,
    used: u64,
}

#[derive(Default)]
pub struct ThumbnailCache {
    entries: HashMap<PathBuf, Entry>,
    bytes: usize,
    tick: u64,
    limit: Option<Arc<smol::lock::Semaphore>>,
    /// Images dropped from the cache, for windows to let go of
    /// (`Window::drop_image`).
    evicted: Vec<Arc<RenderImage>>,
    /// Decodes finished (for tests and `dump`).
    pub decoded: u64,
}

impl Global for ThumbnailCache {}

impl ThumbnailCache {
    /// `path`'s thumbnail, decoding it (and re-rendering `cx`'s view when
    /// it's ready) unless it's cached for this `stamp`.
    pub fn get<V: 'static>(path: &Path, stamp: Stamp, cx: &mut Context<V>) -> State {
        let cache = cx.default_global::<ThumbnailCache>();
        cache.tick += 1;
        let tick = cache.tick;
        if let Some(entry) = cache.entries.get_mut(path)
            && entry.stamp == stamp
        {
            entry.used = tick;
            return entry.state.clone();
        }
        if let Some(Entry { state: State::Ready(old), .. }) = cache.entries.remove(path) {
            cache.bytes -= old.bytes;
            cache.evicted.push(old.image.clone());
        }
        cache.entries.insert(path.to_path_buf(), Entry { stamp, state: State::Loading, used: tick });
        let limit = cache.limit.get_or_insert_with(|| Arc::new(smol::lock::Semaphore::new(CONCURRENCY))).clone();
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let file = path.clone();
            let result = cx
                .background_spawn(async move {
                    let _permit = limit.acquire().await;
                    decode(&file, MAX_PIXELS).map(|d| {
                        let bytes = d.rgba.len();
                        let mut bgra = d.rgba;
                        for pixel in bgra.chunks_exact_mut(4) {
                            pixel.swap(0, 2);
                        }
                        let frame = image::Frame::new(image::RgbaImage::from_raw(d.width, d.height, bgra).expect("sized"));
                        Thumbnail { image: Arc::new(RenderImage::new(vec![frame])), source: d.source, bytes }
                    })
                })
                .await;
            cx.update(|cx| {
                let cache = cx.default_global::<ThumbnailCache>();
                cache.decoded += 1;
                // Asked again for a newer file meanwhile: that one wins.
                let Some(entry) = cache.entries.get_mut(&path).filter(|e| e.stamp == stamp) else { return };
                entry.state = match result {
                    Ok(thumbnail) => {
                        cache.bytes += thumbnail.bytes;
                        State::Ready(Arc::new(thumbnail))
                    }
                    Err(err) => State::Failed(err),
                };
                cache.evict();
            });
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        State::Loading
    }

    fn evict(&mut self) {
        if self.bytes <= BUDGET {
            return;
        }
        let mut ready: Vec<(u64, PathBuf)> =
            self.entries.iter().filter(|(_, e)| matches!(e.state, State::Ready(_))).map(|(p, e)| (e.used, p.clone())).collect();
        ready.sort();
        for (_, path) in ready {
            if self.bytes <= BUDGET * 3 / 4 {
                break;
            }
            if let Some(Entry { state: State::Ready(thumbnail), .. }) = self.entries.remove(&path) {
                self.bytes -= thumbnail.bytes;
                self.evicted.push(thumbnail.image.clone());
            }
        }
    }

    /// Lets `window` free the textures of dropped thumbnails.
    pub fn release(window: &mut Window, cx: &mut App) {
        let Some(cache) = cx.try_global::<ThumbnailCache>() else { return };
        if cache.evicted.is_empty() {
            return;
        }
        for image in std::mem::take(&mut cx.global_mut::<ThumbnailCache>().evicted) {
            window.drop_image(image).ok();
        }
    }

    /// `path`'s thumbnail, without asking for it.
    pub fn peek(&self, path: &Path) -> Option<&State> {
        self.entries.get(path).map(|e| &e.state)
    }

    /// How many thumbnails are kept, and their bytes.
    pub fn usage(cx: &App) -> (usize, usize, u64) {
        cx.try_global::<ThumbnailCache>().map(|c| (c.entries.len(), c.bytes, c.decoded)).unwrap_or_default()
    }
}

/// Decodes `path` scaled to fit `max` pixels (vector images are drawn at
/// that size, larger or smaller). Blocking.
pub fn decode(path: &Path, max: u32) -> Result<Decoded, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "tif" | "tiff" | "bmp" | "webp" | "ico" => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            decode_raster(&bytes, max)
        }
        "svg" => decode_svg(&std::fs::read(path).map_err(|e| e.to_string())?, max),
        "pdf" => decode_pdf(&std::fs::read(path).map_err(|e| e.to_string())?, max),
        _ => decode_with_sips(path, max),
    }
}

fn decode_raster(bytes: &[u8], max: u32) -> Result<Decoded, String> {
    use image::ImageDecoder as _;
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    let source = (image.width(), image.height());
    let image = if source.0 > max || source.1 > max { image.thumbnail(max, max) } else { image };
    let rgba = image.into_rgba8();
    Ok(Decoded { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw(), source })
}

fn fit(width: f32, height: f32, max: u32) -> (u32, u32, f32) {
    let scale = max as f32 / width.max(height).max(1.);
    (((width * scale).round() as u32).max(1), ((height * scale).round() as u32).max(1), scale)
}

/// Premultiplied RGBA (what Core Graphics and tiny-skia draw) to straight.
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let a = pixel[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut pixel[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

fn decode_svg(bytes: &[u8], max: u32) -> Result<Decoded, String> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (width, height, scale) = fit(size.width(), size.height(), max);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or("the SVG has no size")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    unpremultiply(&mut rgba);
    Ok(Decoded { rgba, width, height, source: (size.width().round() as u32, size.height().round() as u32) })
}

/// The first page, drawn by Core Graphics into a bitmap.
fn decode_pdf(bytes: &[u8], max: u32) -> Result<Decoded, String> {
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider, CGImageAlphaInfo, CGImageByteOrderInfo, CGPDFBox, CGPDFDocument, CGPDFPage};

    let data = CFData::from_bytes(bytes);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("can’t read the PDF")?;
    let document = CGPDFDocument::with_provider(Some(&provider)).ok_or("not a PDF")?;
    let page = CGPDFDocument::page(Some(&document), 1).ok_or("the PDF has no pages")?;
    let rect = CGPDFPage::box_rect(Some(&page), CGPDFBox::CropBox);
    let (w, h) = (rect.size.width as f32, rect.size.height as f32);
    if w <= 0. || h <= 0. {
        return Err("the PDF’s page is empty".into());
    }
    let (width, height, scale) = fit(w, h, max);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let space = CGColorSpace::new_device_rgb().ok_or("no RGB color space")?;
    // SAFETY: `rgba` holds `height` rows of `width * 4` bytes and outlives
    // the context, which is dropped below before it's read.
    let context = unsafe {
        CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }
    .ok_or("can’t make a bitmap")?;
    CGContext::scale_ctm(Some(&context), scale as f64, scale as f64);
    CGContext::translate_ctm(Some(&context), -rect.origin.x, -rect.origin.y);
    CGContext::draw_pdf_page(Some(&context), Some(&page));
    drop(context);
    unpremultiply(&mut rgba);
    Ok(Decoded { rgba, width, height, source: (w.round() as u32, h.round() as u32) })
}

/// Through macOS's own image reading (`sips`), as a PNG in a temporary
/// file, for formats the `image` crate doesn't read.
fn decode_with_sips(path: &Path, max: u32) -> Result<Decoded, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let out = std::env::temp_dir().join(format!("zcode-thumb-{}-{}.png", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let status = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", &max.to_string()])
        .arg(path)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&out);
    std::fs::remove_file(&out).ok();
    if !status.success() {
        return Err("No preview".into());
    }
    let mut decoded = decode_raster(&bytes.map_err(|e| e.to_string())?, max)?;
    // `sips -Z` already scaled it; the source size is the file's own.
    if let Some(size) = sips_size(path) {
        decoded.source = size;
    }
    Ok(decoded)
}

fn sips_size(path: &Path) -> Option<(u32, u32)> {
    let out = std::process::Command::new("/usr/bin/sips").args(["-g", "pixelWidth", "-g", "pixelHeight"]).arg(path).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let value = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key)).and_then(|v| v.trim().parse().ok());
    Some((value("pixelWidth:")?, value("pixelHeight:")?))
}

#[cfg(test)]
mod tests {
    use super::{Decoded, decode};
    use crate::xcassets::tests::fixture_catalog;

    fn pixel(d: &Decoded, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * d.width + x) * 4) as usize;
        [d.rgba[i], d.rgba[i + 1], d.rgba[i + 2], d.rgba[i + 3]]
    }

    #[test]
    fn raster_images_are_scaled_down_only() {
        let logo = fixture_catalog().join("Logo.imageset/logo@2x.png");
        let d = decode(&logo, 192).unwrap();
        assert_eq!((d.width, d.height, d.source), (64, 64, (64, 64)));
        assert_eq!(pixel(&d, 10, 10), [52, 199, 89, 255]);
        let small = decode(&logo, 16).unwrap();
        assert_eq!((small.width, small.height, small.source), (16, 16, (64, 64)));
    }

    #[test]
    fn pdfs_are_drawn_by_core_graphics() {
        let badge = fixture_catalog().join("Icons/Badge.imageset/badge.pdf");
        let d = decode(&badge, 64).unwrap();
        assert_eq!((d.width, d.height, d.source), (64, 64, (32, 32)));
        // A blue square inset by 4 of 32 points: transparent at the corner.
        assert_eq!(pixel(&d, 1, 1)[3], 0);
        let [r, g, b, a] = pixel(&d, 32, 32);
        assert_eq!(a, 255);
        assert!(b > 180 && r < 90 && g > 120, "{:?}", [r, g, b]);
    }

    #[test]
    fn svgs_are_drawn_by_resvg() {
        let star = fixture_catalog().join("Icons/Star.symbolset/star.svg");
        let d = decode(&star, 96).unwrap();
        assert_eq!((d.width, d.height, d.source), (96, 96, (32, 32)));
        assert_eq!(pixel(&d, 48, 48), [255, 204, 0, 255]);
        assert_eq!(pixel(&d, 0, 95)[3], 0);
    }

    #[test]
    fn other_formats_through_sips() {
        let dir = std::env::temp_dir().join(format!("zcode-thumb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let heic = dir.join("logo.heic");
        let made = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(fixture_catalog().join("Logo.imageset/logo@2x.png"))
            .arg("--out")
            .arg(&heic)
            .output()
            .is_ok_and(|o| o.status.success());
        if made {
            let d = decode(&heic, 32).unwrap();
            assert_eq!((d.width, d.height, d.source), (32, 32, (64, 64)));
        }
        assert!(decode(&dir.join("missing.xyz"), 32).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
