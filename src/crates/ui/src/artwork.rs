use crate::metrics::snapped;
use crate::palette::{CoverPalette, of_image};
use crate::skeleton::Skeleton;
use crate::theme::{ActiveTheme as _, legible};
use base64::Engine as _;
use futures::AsyncReadExt as _;
use futures::channel::oneshot;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Asset, AssetLogger, Bounds, Context, Div, ElementId, Entity, Global,
    GlobalElementId, Hsla, ImageCache, ImageCacheError, ImageSource, InspectorElementId,
    Interactivity, LayoutId, ObjectFit, Pixels, Position, RenderImage, Resource, SharedString,
    SharedUri, Style, StyleRefinement, Styled, Task, Window, div, img, px, svg,
};
use image::{
    AnimationDecoder, DynamicImage, Frame, ImageDecoder, ImageFormat, Rgba, RgbaImage,
    codecs::{gif::GifDecoder, webp::WebPDecoder},
    imageops,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Cursor;
use std::panic::Location;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

const FILE_PREFIX: &str = "file://";

const FALLBACK_ICON: &str = "icons/music.svg";
pub(crate) const ROUNDED: Pixels = px(4.);
/// What the cache trims back to at the least. The budget grows to one screenful of the
/// largest window a cover was drawn in, plus the `PREFETCH_REACH` laid out past both of its
/// edges, so a HiDPI grid never trims what it shows or is about to. The cache is allowed half
/// as much again past its budget while a scroll pulls covers in, since a trim asks every
/// window to redraw and is worth doing in one batch.
const CACHE_BYTES: usize = 16 * 1024 * 1024;
const CACHE_ITEMS: usize = 256;
/// The largest edge a cover is decoded at. It sits above a fullscreen cover on a 5K screen at
/// 2x, so it only ever stops a local original of several thousand pixels from being kept whole.
const MAX_SAMPLE_EDGE: u32 = 4096;
/// The step decode edges are rounded up to. A power of two would decode up to four times
/// the pixels drawn.
const EDGE_STEP: u32 = 64;
/// The edge a cover is decoded at when only its palette is wanted.
const PALETTE_EDGE: u32 = 128;
/// Set on a decode edge when the image keeps its own aspect and is scaled to that width, for
/// banners that a square crop would cut down. A wide decode never stands in for a square one.
const WIDE: u32 = 1 << 31;
const GRACE: Duration = Duration::from_secs(5);
const KEEP_ITEMS: usize = 96;
const IDLE: Duration = Duration::from_secs(120);
const ORPHAN: Duration = Duration::from_secs(20);
const SWEEP: Duration = Duration::from_secs(30);
/// How many softened covers are kept. Only the cover waiting on its large art is drawn
/// soft, so two cover the current track and a skip back.
const SOFT_ITEMS: usize = 2;
const SOFT_SIGMA: f32 = 1.6;
const SMALL_BYTES: usize = 64 * 1024;
const BIG_BYTES: usize = 256 * 1024;
/// How many covers may be loading at once. The FM stack shows thirty covers, so a whole
/// stack is let in together rather than a few at a time.
const MAX_PENDING: usize = 32;
/// How many of the `MAX_PENDING` loads a prefetched cover may take, so a cover on screen always
/// finds a free one.
const PREFETCH_PENDING: usize = MAX_PENDING / 2;
/// How far past each edge of a scrolled region rows are laid out unseen so their covers load
/// before they scroll in, as a share of the region's length.
pub const PREFETCH_REACH: f32 = 0.5;
/// How many covers decode at once. A decode holds the whole source image, so this bounds
/// the transient memory, and running decodes on their own threads keeps those buffers in a
/// couple of malloc arenas rather than one per worker thread.
const DECODERS: usize = 2;
/// How long a condemned cover is held before it is dropped. One redraw of every window
/// is all it takes for anything still on screen to ask for its cover again, and that
/// redraw is already on its way when the batch is condemned.
const REPRIEVE: Duration = Duration::from_millis(250);
/// How many cover palettes are kept. Each one is two colours, so the map costs
/// nothing beside the frames, and holding them past an eviction is what keeps a
/// button its colour while its cover is decoded again.
const TINT_ITEMS: usize = 4096;

type ArtworkKey = (Resource, u32);

thread_local! {
    /// Set while `prefetch` lays out rows that will not be drawn.
    static PREFETCHING: Cell<bool> = const { Cell::new(false) };
}

type Job = Box<dyn FnOnce() + Send>;

#[derive(Clone, Hash)]
struct ArtworkSource {
    resource: Resource,
    edge: u32,
}

#[derive(Clone)]
enum ArtworkAssetLoader {}

type ArtworkResourceLoader = AssetLogger<ArtworkAssetLoader>;

/// Reads the encoded bytes of a cover. Nothing holds on to them once the decode has
/// landed, because the HTTP disk cache makes a second read cheap.
fn fetch(
    resource: Resource,
    cx: &App,
) -> impl std::future::Future<Output = Result<Vec<u8>, ImageCacheError>> + Send + 'static {
    let client = cx.http_client();
    let asset_source = cx.asset_source().clone();

    async move {
        match resource {
            Resource::Path(path) => Ok(std::fs::read(path.as_ref())?),
            Resource::Uri(uri) => {
                let mut response = client.get(uri.as_ref(), ().into(), true).await?;
                let mut body = Vec::new();
                response.body_mut().read_to_end(&mut body).await?;
                if !response.status().is_success() {
                    let mut body = String::from_utf8_lossy(&body).into_owned();
                    let first_line = body.lines().next().unwrap_or("").trim_end();
                    body.truncate(first_line.len());
                    return Err(ImageCacheError::BadStatus {
                        uri,
                        status: response.status(),
                        body,
                    });
                }
                Ok(body)
            }
            Resource::Embedded(path) => {
                let Some(data) = asset_source.load(&path)? else {
                    return Err(ImageCacheError::Asset(
                        format!("Embedded resource not found: {path}").into(),
                    ));
                };
                Ok(data.into_owned())
            }
        }
    }
}

/// A decoded cover with the palette sampled from it. The loader samples it on its own
/// thread, so a cover landing mid-scroll costs the frame nothing beyond storing it.
#[derive(Clone)]
struct Decoded {
    image: Arc<RenderImage>,
    palette: CoverPalette,
}

impl Asset for ArtworkAssetLoader {
    type Source = ArtworkSource;
    type Output = Result<Decoded, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let svg_renderer = cx.svg_renderer();
        let bytes = fetch(source.resource, cx);

        async move {
            let bytes = bytes.await?;

            let Ok(format) = image::guess_format(&bytes) else {
                let image = svg_renderer.render_single_frame(&bytes, 1.0)?;
                let palette = of_image(&image);
                return Ok(Decoded { image, palette });
            };
            on_decoder(move || {
                let image = Arc::new(RenderImage::new(raster_frames(
                    &bytes,
                    format,
                    source.edge,
                )?));
                let palette = of_image(&image);
                Ok(Decoded { image, palette })
            })
            .await
            .unwrap_or_else(|| Err(ImageCacheError::Asset("artwork decoder stopped".into())))
        }
    }
}

/// Runs `decode` on one of the `DECODERS` artwork threads and resolves with its result, or
/// with none when the job died with its thread. A job whose caller stopped waiting is skipped.
fn on_decoder<T: Send + 'static>(
    decode: impl FnOnce() -> T + Send + 'static,
) -> impl std::future::Future<Output = Option<T>> + Send + 'static {
    static QUEUE: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

    let (sender, receiver) = oneshot::channel();
    let job: Job = Box::new(move || {
        if !sender.is_canceled() {
            sender.send(decode()).ok();
        }
    });
    if let Err(mpsc::SendError(job)) = QUEUE.get_or_init(decoders).send(job) {
        job();
    }

    async move { receiver.await.ok() }
}

/// Starts the artwork decode threads and hands back the queue that feeds them. When none
/// could start, the queue refuses every job and `on_decoder` runs it in place.
fn decoders() -> mpsc::Sender<Job> {
    let (sender, receiver) = mpsc::channel::<Job>();
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..DECODERS {
        let receiver = receiver.clone();
        let started = std::thread::Builder::new()
            .name(format!("artwork-{index}"))
            .spawn(move || {
                loop {
                    let next = receiver.lock().ok().and_then(|queue| queue.recv().ok());
                    let Some(job) = next else {
                        return;
                    };
                    job();
                }
            });
        if let Err(error) = started {
            log::warn!("artwork: cannot start a decoder: {error}");
        }
    }

    sender
}

fn raster_frames(
    bytes: &[u8],
    format: ImageFormat,
    edge: u32,
) -> Result<Vec<Frame>, ImageCacheError> {
    match format {
        ImageFormat::Gif => animated_frames(GifDecoder::new(Cursor::new(bytes))?, edge),
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
            if decoder.has_animation() {
                let _ = decoder.set_background_color(image::Rgba([0, 0, 0, 0]));
                animated_frames(decoder, edge)
            } else {
                static_frame(decoder, edge)
            }
        }
        _ => {
            let decoder =
                image::ImageReader::with_format(Cursor::new(bytes), format).into_decoder()?;
            static_frame(decoder, edge)
        }
    }
}

fn static_frame(mut decoder: impl ImageDecoder, edge: u32) -> Result<Vec<Frame>, ImageCacheError> {
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(vec![Frame::new(artwork_frame(image, edge))])
}

fn animated_frames<'a>(
    decoder: impl AnimationDecoder<'a>,
    edge: u32,
) -> Result<Vec<Frame>, ImageCacheError> {
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        match frame {
            Ok(frame) => {
                let delay = frame.delay();
                frames.push(Frame::from_parts(
                    artwork_frame(DynamicImage::ImageRgba8(frame.into_buffer()), edge),
                    0,
                    0,
                    delay,
                ));
            }
            Err(error) => log::debug!("Skipping artwork animation frame: {error}"),
        }
    }
    if frames.is_empty() {
        return Err(ImageCacheError::Asset(
            "Animated artwork contained no decodable frames".into(),
        ));
    }
    Ok(frames)
}

/// Crops a decoded cover to its centre square, scales it down to `edge` and swaps it to
/// BGRA. The scaling happens in the decoder's own pixel format, so an RGB JPEG only grows
/// a fourth channel at the size it is drawn. An image no bigger than `edge`, or any image
/// when `edge` is zero, keeps its size. An edge flagged `WIDE` keeps the whole image and
/// scales its width down to the rest of the edge.
fn artwork_frame(image: DynamicImage, edge: u32) -> RgbaImage {
    let (width, height) = (image.width(), image.height());
    if edge & WIDE != 0 {
        let wide = edge & !WIDE;
        let image = match width <= wide {
            true => image,
            false => image.resize(wide, u32::MAX, imageops::FilterType::Triangle),
        };
        let mut image = image.into_rgba8();
        bgra(&mut image);
        return image;
    }
    let side = width.min(height);
    let image = match edge == 0 || side <= edge {
        true => image,
        false => {
            let square = match width == height {
                true => image,
                false => image.crop_imm((width - side) / 2, (height - side) / 2, side, side),
            };
            match side > edge.saturating_mul(2) {
                true => square.thumbnail_exact(edge, edge),
                false => square.resize_exact(edge, edge, imageops::FilterType::Triangle),
            }
        }
    };
    let mut image = image.into_rgba8();
    bgra(&mut image);
    image
}

fn bgra(image: &mut RgbaImage) {
    for pixel in image.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

struct Cached {
    value: Result<Arc<RenderImage>, ImageCacheError>,
    bytes: usize,
    used: Instant,
}

struct ArtworkCache {
    items: HashMap<ArtworkKey, Cached>,
    /// Covers taken out of `items` and kept alive until the windows have redrawn once. See
    /// `condemn`.
    condemned: HashMap<ArtworkKey, Cached>,
    condemned_soft: Vec<Arc<RenderImage>>,
    condemned_at: Option<Instant>,
    pending: HashMap<ArtworkKey, Instant>,
    soft: HashMap<(Resource, u32), Arc<RenderImage>>,
    /// The palette of every cover decoded this run, kept apart from the frames
    /// so an eviction never costs a button its colour.
    tints: HashMap<Resource, CoverPalette>,
    bytes: usize,
    /// The size in bytes of the largest window a cover was drawn in, which sets the budget.
    screen: usize,
    _sweep: Task<()>,
}

struct Installed(Entity<ArtworkCache>);

impl Global for Installed {}

impl ArtworkCache {
    fn entity(cx: &mut App) -> Entity<Self> {
        if cx.try_global::<Installed>().is_none() {
            let cache = cx.new(|cx| Self {
                items: HashMap::new(),
                condemned: HashMap::new(),
                condemned_soft: Vec::new(),
                condemned_at: None,
                pending: HashMap::new(),
                soft: HashMap::new(),
                tints: HashMap::new(),
                bytes: 0,
                screen: 0,
                _sweep: sweeper(cx),
            });
            cx.set_global(Installed(cache));
        }
        cx.global::<Installed>().0.clone()
    }

    fn insert(
        &mut self,
        resource: ArtworkKey,
        decoded: Result<Decoded, ImageCacheError>,
        cx: &mut App,
    ) {
        if let Ok(decoded) = &decoded
            && !self.tints.contains_key(&resource.0)
        {
            self.trim_tints();
            self.tints.insert(resource.0.clone(), decoded.palette);
        }
        let value = decoded.map(|decoded| decoded.image);
        let bytes = value.as_ref().map_or(0, |image| image_bytes(image));
        self.bytes = self.bytes.saturating_add(bytes);
        self.items.insert(
            resource,
            Cached {
                value,
                bytes,
                used: Instant::now(),
            },
        );

        self.trim(cx);
    }

    /// Drops the palettes of covers no longer held, once the map has grown past
    /// its cap. Scrolling through more art than that pays one pass.
    fn trim_tints(&mut self) {
        if self.tints.len() < TINT_ITEMS {
            return;
        }
        let live = &self.items;
        self.tints
            .retain(|resource, _| live.keys().any(|key| &key.0 == resource));
    }

    fn oldest(&self) -> Option<(ArtworkKey, Instant)> {
        self.items
            .iter()
            .min_by_key(|(_, cached)| cached.used)
            .map(|(resource, cached)| (resource.clone(), cached.used))
    }

    /// Takes a cover out of the cache without dropping it. Dropping one frees its tile in
    /// the GPU atlas, and a view whose layout was cached redraws from the primitives it
    /// recorded last frame, so a cover still shown there would be painted with whatever art
    /// took its tile over. A condemned cover keeps its frames until `flush`, and `load_at`
    /// takes it back the moment anything asks for it again.
    fn condemn(&mut self, resource: &ArtworkKey) {
        let Some(cached) = self.items.remove(resource) else {
            return;
        };
        self.bytes = self.bytes.saturating_sub(cached.bytes);
        self.condemned.insert(resource.clone(), cached);
    }

    /// What the cache trims back to: `CACHE_BYTES`, or one screenful of the largest window and
    /// its prefetched reach when that is more.
    fn budget(&self) -> usize {
        let reach = (self.screen as f32 * PREFETCH_REACH * 2.) as usize;
        CACHE_BYTES.max(self.screen + reach)
    }

    /// Condemns the least recently drawn covers until the cache is back inside its budget,
    /// then asks every window to redraw. The redraw is what makes the condemned batch safe
    /// to drop: it rebuilds every cached view, so nothing is replayed from last frame's
    /// primitives and everything still on screen asks for its cover again.
    fn trim(&mut self, cx: &mut App) {
        let budget = self.budget();
        if self.bytes <= budget + budget / 2 && self.items.len() <= CACHE_ITEMS {
            return;
        }
        let before = self.condemned.len();
        while self.items.len() > 1 && (self.bytes > budget || self.items.len() > CACHE_ITEMS) {
            let Some((resource, _)) = self.oldest() else {
                break;
            };
            self.condemn(&resource);
        }
        if self.condemned.len() > before {
            self.condemned_at = Some(Instant::now());
            cx.refresh_windows();
        }
    }

    /// Drops every cover still condemned once the redraw that `trim` asked for has been and
    /// gone. Whatever is left here was on no screen through a full rebuild of every view.
    fn flush(&mut self, cx: &mut App) {
        if self.condemned_at.is_none_or(|at| at.elapsed() < REPRIEVE) {
            return;
        }
        self.condemned_at = None;
        for (resource, cached) in std::mem::take(&mut self.condemned) {
            cx.remove_asset::<ArtworkResourceLoader>(&ArtworkSource {
                resource: resource.0.clone(),
                edge: resource.1,
            });
            if let Ok(image) = cached.value {
                cx.drop_image(image, None);
            }
        }
        for image in std::mem::take(&mut self.condemned_soft) {
            cx.drop_image(image, None);
        }
    }

    fn prepared(&self, resource: &Resource, edge: u32, soft: bool) -> Option<Arc<RenderImage>> {
        match soft {
            true => self.soft.get(&(resource.clone(), edge)).cloned(),
            false => None,
        }
    }

    fn prepare(
        &mut self,
        resource: &Resource,
        edge: u32,
        soft: bool,
        image: Arc<RenderImage>,
        cx: &mut App,
    ) -> Arc<RenderImage> {
        if !soft {
            return image;
        }

        let key = (resource.clone(), edge);
        if let Some(found) = self.soft.get(&key) {
            return found.clone();
        }
        if self.soft.len() >= SOFT_ITEMS {
            self.condemned_soft
                .extend(self.soft.drain().map(|(_, image)| image));
            self.condemned_at = Some(Instant::now());
            cx.refresh_windows();
        }
        let Some(softened) = blurred(&image) else {
            return image;
        };
        self.soft.insert(key, softened.clone());
        softened
    }

    fn sweep(&mut self, cx: &mut App) {
        let held = self.items.len();
        let abandoned: Vec<ArtworkKey> = self
            .pending
            .iter()
            .filter(|(_, started)| started.elapsed() > ORPHAN)
            .map(|(resource, _)| resource.clone())
            .collect();

        for resource in &abandoned {
            self.pending.remove(resource);
            cx.remove_asset::<ArtworkResourceLoader>(&ArtworkSource {
                resource: resource.0.clone(),
                edge: resource.1,
            });
        }

        let mut ages: Vec<(ArtworkKey, Instant, usize)> = self
            .items
            .iter()
            .map(|(resource, cached)| (resource.clone(), cached.used, cached.bytes))
            .collect();
        ages.sort_unstable_by_key(|(_, used, _)| *used);

        let idle = ages
            .iter()
            .filter(|(_, used, _)| used.elapsed() > IDLE)
            .count();
        let protected = ages.len().saturating_sub(KEEP_ITEMS);
        let budget = self.budget();
        let mut bytes = self.bytes;
        let mut stale = Vec::new();

        for (index, (resource, used, size)) in ages.iter().enumerate() {
            if index >= protected || used.elapsed() <= GRACE {
                break;
            }
            if bytes <= budget && used.elapsed() <= IDLE {
                break;
            }
            stale.push(resource.clone());
            bytes = bytes.saturating_sub(*size);
        }

        for resource in &stale {
            self.condemn(resource);
        }
        if !stale.is_empty() {
            self.condemned_at = Some(Instant::now());
            cx.refresh_windows();
        }

        let tiny = self.count(..SMALL_BYTES);
        let small = self.count(SMALL_BYTES..BIG_BYTES);
        let big = self.count(BIG_BYTES..);

        log::debug!(
            "artwork: {} held / {} KiB, dropped {}, idle {idle}, abandoned {}, waiting {}, sizes {tiny}/{small}/{big}",
            self.items.len(),
            self.bytes / 1024,
            held - self.items.len(),
            abandoned.len(),
            self.pending.len()
        );
    }

    fn count(&self, range: impl std::ops::RangeBounds<usize>) -> usize {
        self.items
            .values()
            .filter(|cached| range.contains(&cached.bytes))
            .count()
    }
}

/// Releases condemned covers as their reprieve runs out and sweeps the cache on its own
/// slower clock. The two share one timer because a tick that finds nothing to release costs
/// a look at an empty map.
fn sweeper(cx: &mut Context<ArtworkCache>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let mut swept = Instant::now();
        loop {
            cx.background_executor().timer(REPRIEVE).await;
            let due = swept.elapsed() >= SWEEP;
            let alive = this.update(cx, |this, cx| {
                this.flush(cx);
                if due {
                    this.sweep(cx);
                }
            });
            if alive.is_err() {
                return;
            }
            if due {
                swept = Instant::now();
            }
        }
    })
}

impl ImageCache for ArtworkCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.load_at(resource, 0, window, cx)
    }
}

impl ArtworkCache {
    fn reap_pending(&mut self, window: &mut Window, cx: &mut App) {
        let pending: Vec<ArtworkKey> = self.pending.keys().cloned().collect();
        for key in pending {
            let source = ArtworkSource {
                resource: key.0.clone(),
                edge: key.1,
            };
            let Some(value) = window.use_asset::<ArtworkResourceLoader>(&source, cx) else {
                continue;
            };
            self.pending.remove(&key);
            self.insert(key, value, cx);
        }
    }

    fn load_at(
        &mut self,
        resource: &Resource,
        edge: u32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let key = (resource.clone(), edge);
        if let Some(cached) = self.items.get_mut(&key) {
            cached.used = Instant::now();
            return Some(cached.value.clone());
        }
        // Still on screen after all, so it goes back in the cache rather than being dropped.
        if let Some(mut cached) = self.condemned.remove(&key) {
            cached.used = Instant::now();
            let value = cached.value.clone();
            self.bytes = self.bytes.saturating_add(cached.bytes);
            self.items.insert(key, cached);
            return Some(value);
        }
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        let screen =
            (viewport.width.as_f32() * viewport.height.as_f32() * scale * scale) as usize * 4;
        self.screen = self.screen.max(screen);

        let room = match PREFETCHING.get() {
            true => PREFETCH_PENDING,
            false => MAX_PENDING,
        };
        if !self.pending.contains_key(&key) && self.pending.len() >= room {
            self.reap_pending(window, cx);
            if self.pending.len() >= room {
                return None;
            }
        }

        let source = ArtworkSource {
            resource: resource.clone(),
            edge,
        };
        let Some(value) = window.use_asset::<ArtworkResourceLoader>(&source, cx) else {
            self.pending.insert(key, Instant::now());
            return None;
        };

        let image = value.clone().map(|decoded| decoded.image);
        self.pending.remove(&key);
        self.insert(key, value, cx);
        Some(image)
    }

    /// Whether a cover at `edge` can be drawn without waiting on a load, counting a failed load,
    /// which draws the placeholder, and a stand-in at a nearby edge.
    fn holds(&self, resource: &Resource, edge: u32, soft: bool) -> bool {
        let key = (resource.clone(), edge);
        if self.items.contains_key(&key) || self.condemned.contains_key(&key) {
            return true;
        }
        match soft {
            true => self.soft.contains_key(&key),
            false => self.items.iter().any(|((held, size), cached)| {
                held == resource && near(*size, edge) && cached.value.is_ok()
            }),
        }
    }

    /// The largest decode the cache holds of the same cover at no less than half of `edge`, to
    /// draw while the one at `edge` loads. A resize that moves a cover to the next edge step
    /// keeps it on screen instead of flashing the skeleton.
    fn stand_in(&mut self, resource: &Resource, edge: u32) -> Option<Arc<RenderImage>> {
        let (_, cached) = self
            .items
            .iter_mut()
            .filter(|((held, size), cached)| {
                held == resource && near(*size, edge) && cached.value.is_ok()
            })
            .max_by_key(|((_, size), _)| *size)?;
        cached.used = Instant::now();
        cached.value.clone().ok()
    }
}

fn blurred(image: &RenderImage) -> Option<Arc<RenderImage>> {
    let frames: Vec<Frame> = (0..image.frame_count())
        .filter_map(|index| {
            let size = image.size(index);
            let width = size.width.0.max(0) as u32;
            let height = size.height.0.max(0) as u32;
            let bytes = image.as_bytes(index)?.to_vec();
            let whole = RgbaImage::from_raw(width, height, bytes)?;

            Some(Frame::from_parts(
                imageops::fast_blur(&whole, SOFT_SIGMA),
                0,
                0,
                image.delay(index),
            ))
        })
        .collect();
    if frames.len() != image.frame_count() {
        log::warn!("artwork: cannot soften an image");
        return None;
    }

    Some(Arc::new(RenderImage::new(frames)))
}

/// The edge a cover drawn at `size` is decoded at: its physical size rounded up to the next
/// `EDGE_STEP`, so nearby sizes share one decode, and never past `MAX_SAMPLE_EDGE`.
fn sample_edge(size: Pixels, window: &Window) -> u32 {
    let physical = ((size / px(1.)) * window.scale_factor()).ceil().max(1.) as u32;
    physical.next_multiple_of(EDGE_STEP).min(MAX_SAMPLE_EDGE)
}

/// Whether a decode at `held` may stand in for one at `edge`: the same shape, square or wide,
/// at no less than half the size.
fn near(held: u32, edge: u32) -> bool {
    held & WIDE == edge & WIDE && held & !WIDE >= (edge & !WIDE) / 2
}

pub(crate) fn resource(url: impl Into<SharedString>) -> Resource {
    let url = url.into();
    match url.strip_prefix(FILE_PREFIX) {
        Some(path) => Resource::Path(Arc::from(Path::new(path))),
        None => Resource::Uri(SharedUri::from(url)),
    }
}

/// The palette of a cover the artwork cache has already decoded, or none while
/// it has not been drawn yet. Nothing is decoded here, so the colours land on
/// the frame the cover appears and never before it.
pub fn cover_palette(url: &str, cx: &App) -> Option<CoverPalette> {
    let installed = cx.try_global::<Installed>()?;
    let resource = resource(url.to_owned());

    installed.0.read(cx).tints.get(&resource).copied()
}

/// The palette of the cover at `url` without keeping its pixels. A cover the cache has
/// drawn answers from its stored palette, and any other is decoded at `PALETTE_EDGE`,
/// which is plenty for a hue count and costs a fraction of the full-size frame.
pub(crate) fn sample_palette(url: SharedString, cx: &mut App) -> Task<CoverPalette> {
    if let Some(palette) = cover_palette(&url, cx) {
        return Task::ready(palette);
    }
    let load = ArtworkAssetLoader::load(
        ArtworkSource {
            resource: resource(url),
            edge: PALETTE_EDGE,
        },
        cx,
    );

    cx.background_spawn(async move {
        load.await
            .map(|decoded| decoded.palette)
            .unwrap_or_default()
    })
}

/// Decodes a `data:image/...;base64,...` URI into an image an element can draw directly. Used
/// for QR codes a platform mints itself, which cannot be re-encoded from a URL.
pub fn data_uri_image(uri: &str) -> Option<Arc<RenderImage>> {
    let (header, data) = uri.split_once(',')?;
    if !header.starts_with("data:image/") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let format = image::guess_format(&bytes).ok()?;
    let frames = raster_frames(&bytes, format, 0).ok()?;
    Some(Arc::new(RenderImage::new(frames)))
}

pub fn artwork_usage(cx: &App) -> Option<(usize, usize)> {
    let installed = cx.try_global::<Installed>()?;
    let cache = installed.0.read(cx);
    let soft_bytes: usize = cache.soft.values().map(|image| image_bytes(image)).sum();
    Some((
        cache.items.len() + cache.soft.len(),
        cache.bytes + soft_bytes,
    ))
}

/// Lays out `element` without drawing it, so the covers inside start loading before it
/// scrolls into view. Those loads take at most `PREFETCH_PENDING` of the slots, so a cover
/// already on screen never waits behind one that is not.
pub fn prefetch(mut element: AnyElement, window: &mut Window, cx: &mut App) {
    let outer = PREFETCHING.replace(true);
    element.request_layout(window, cx);
    PREFETCHING.set(outer);
}

/// Whether `prefetch` is laying out what is being built, which will not be drawn. An element
/// that draws a view through `cached` has to render it plainly then, since a cached view is
/// laid out without rendering and so would load no covers.
pub fn prefetching() -> bool {
    PREFETCHING.get()
}

/// Rows just past the edge of a list, laid out with `prefetch` while the list is, so their
/// covers are loading when they scroll in. It takes no room and draws nothing.
pub struct Prefetch {
    rows: Vec<AnyElement>,
}

impl Prefetch {
    pub fn new(rows: impl IntoIterator<Item = AnyElement>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
        }
    }
}

impl IntoElement for Prefetch {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Prefetch {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        for row in self.rows.drain(..) {
            prefetch(row, window, cx);
        }
        let style = Style {
            position: Position::Absolute,
            ..Style::default()
        };

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

fn image_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .filter_map(|frame| image.as_bytes(frame))
        .fold(0, |bytes, frame| bytes.saturating_add(frame.len()))
}

#[derive(IntoElement)]
pub struct Avatar {
    art: Artwork,
}

impl Avatar {
    #[track_caller]
    pub fn new(url: Option<impl Into<SharedString>>) -> Self {
        Self {
            art: Artwork::new(url).circle().flex_none(),
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.art = self.art.size(size);
        self
    }
}

impl Styled for Avatar {
    fn style(&mut self) -> &mut StyleRefinement {
        self.art.style()
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.art
    }
}

#[derive(IntoElement)]
pub struct Artwork {
    url: Option<SharedString>,
    size: Pixels,
    /// The height of a wide image, which is `size` wide. None keeps the square.
    tall: Option<Pixels>,
    circle: bool,
    radius: Option<Pixels>,
    fallback: SharedString,
    /// A word drawn in place of the fallback icon when there is no cover, as a genre tile does.
    fallback_text: Option<SharedString>,
    accent: bool,
    soft: bool,
    overlay: bool,
    fit: ObjectFit,
    /// The size the image is decoded for, when it differs from the size it is drawn at.
    sample: Option<Pixels>,
    interactivity: Interactivity,
}

impl Artwork {
    #[track_caller]
    pub fn new(url: Option<impl Into<SharedString>>) -> Self {
        Self {
            url: url.map(Into::into),
            size: px(28.),
            tall: None,
            circle: false,
            radius: None,
            soft: false,
            overlay: false,
            fallback: FALLBACK_ICON.into(),
            fallback_text: None,
            accent: false,
            fit: ObjectFit::Cover,
            sample: None,
            interactivity: Interactivity::new(),
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.interactivity.element_id = Some(id.into());
        self
    }

    /// Draws the image `size` wide and `height` tall, keeping its own aspect and covering the
    /// box instead of cropping it to a square first. Meant for banners.
    pub fn wide(mut self, height: Pixels) -> Self {
        self.tall = Some(height);
        self
    }

    pub fn circle(mut self) -> Self {
        self.circle = true;
        self
    }

    /// The corner radius a cover takes when it is given none.
    pub fn rounding(cx: &App) -> Pixels {
        cx.theme().radius.min(ROUNDED)
    }

    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.radius = Some(radius);
        self
    }

    pub fn fallback(mut self, icon: impl Into<SharedString>) -> Self {
        self.fallback = icon.into();
        self
    }

    /// Draws `text` over the placeholder instead of the music glyph, so a coverless genre still
    /// reads as its own tile.
    pub fn fallback_text(mut self, text: impl Into<SharedString>) -> Self {
        self.fallback_text = Some(text.into());
        self
    }

    pub fn soft(mut self, soft: bool) -> Self {
        self.soft = soft;
        self
    }

    /// Keeps the whole image inside the square instead of cropping it to fill, for art whose
    /// edges matter, as in the full-size cover view.
    pub fn contain(mut self) -> Self {
        self.fit = ObjectFit::Contain;
        self
    }

    /// Decodes the image for drawing at `size` and stretches that decode to the drawn size, so a
    /// large copy can reuse the decode a smaller one already holds while its own loads.
    pub fn sampled_at(mut self, size: Pixels) -> Self {
        self.sample = Some(size);
        self
    }

    /// Whether the artwork cache holds `url` decoded for drawing at `size`, or near enough to
    /// stand in for it, for an overlay that fades in only once it has something to show.
    pub fn decoded(url: &str, size: Pixels, window: &Window, cx: &mut App) -> bool {
        let cache = ArtworkCache::entity(cx);
        let edge = sample_edge(snapped(size, window), window);
        cache.read(cx).holds(&resource(url.to_owned()), edge, false)
    }

    /// Draws nothing while the image loads or after it fails, for a cover stacked over another
    /// that already shows, so neither the skeleton nor the placeholder hides it.
    pub fn overlay(mut self) -> Self {
        self.overlay = true;
        self
    }

    pub fn accent(mut self) -> Self {
        self.accent = true;
        self
    }
}

impl Styled for Artwork {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for Artwork {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl RenderOnce for Artwork {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            url,
            size,
            tall,
            circle,
            radius,
            fallback,
            fallback_text,
            accent,
            soft,
            overlay,
            fit,
            sample,
            interactivity,
        } = self;
        let theme = *cx.theme();
        let muted = theme.muted_foreground;
        let glyph = match (accent, fallback_text.is_some()) {
            (true, _) => legible(theme.tint.unwrap_or(theme.primary), theme.background),
            // A genre monogram reads as the tile itself, not as a faint watermark.
            (false, true) => theme.foreground.opacity(0.72),
            (false, false) => muted.opacity(0.5),
        };
        let size = snapped(size, window);
        let wide = tall.is_some();
        let tall = tall.map_or(size, |tall| snapped(tall, window));
        let rounded = match (circle, radius) {
            (true, _) => size / 2.,
            (false, Some(radius)) => radius,
            (false, None) => Self::rounding(cx),
        };
        let placeholder = {
            let fallback = fallback.clone();
            let fallback_text = fallback_text.clone();
            move || match overlay {
                true => div().w(size).h(tall).into_any_element(),
                false => blank(
                    size,
                    tall,
                    rounded,
                    muted,
                    glyph,
                    fallback.clone(),
                    fallback_text.clone(),
                )
                .into_any_element(),
            }
        };

        match url {
            Some(url) => {
                let cache = ArtworkCache::entity(cx);
                let resource = resource(url);
                let sampled = sample.map_or(size, |sample| snapped(sample, window));
                let edge = match wide {
                    true => sample_edge(sampled, window) | WIDE,
                    false => sample_edge(sampled, window),
                };
                if PREFETCHING.get() {
                    cache.update(cx, |cache, cx| cache.load_at(&resource, edge, window, cx));
                    return div().w(size).h(tall).into_any_element();
                }
                // Drawn at once rather than after `img`'s loading delay, and under the cover,
                // so a cover that lands mid-pulse simply covers it.
                let waiting = !overlay && !cache.read(cx).holds(&resource, edge, soft);
                let source = ImageSource::Custom(Arc::new({
                    let cache = cache.clone();
                    move |window, cx| {
                        if let Some(prepared) =
                            cache.update(cx, |cache, _| cache.prepared(&resource, edge, soft))
                        {
                            return Some(Ok(prepared));
                        }
                        let Some(loaded) = cache
                            .update(cx, |cache, cx| cache.load_at(&resource, edge, window, cx))
                        else {
                            return match soft {
                                true => None,
                                false => cache
                                    .update(cx, |cache, _| cache.stand_in(&resource, edge))
                                    .map(Ok),
                            };
                        };
                        let loaded = loaded.map(|image| {
                            cache.update(cx, |cache, cx| {
                                cache.prepare(&resource, edge, soft, image, cx)
                            })
                        });
                        Some(loaded)
                    }
                }));
                refined(
                    div()
                        .relative()
                        .w(size)
                        .h(tall)
                        .when(waiting, |this| {
                            this.child(
                                Skeleton::new()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .w(size)
                                    .h(tall)
                                    .rounded(rounded),
                            )
                        })
                        .child(
                            img(source)
                                .image_cache(&cache)
                                .w(size)
                                .h(tall)
                                .object_fit(fit)
                                .rounded(rounded)
                                .with_fallback(placeholder),
                        ),
                    interactivity,
                )
                .into_any_element()
            }
            None => refined(
                blank(size, tall, rounded, muted, glyph, fallback, fallback_text),
                interactivity,
            )
            .into_any_element(),
        }
    }
}

fn refined<T: Styled + InteractiveElement>(mut element: T, mut caller: Interactivity) -> T {
    let mut style = std::mem::take(element.style());
    style.refine(&caller.base_style);
    *caller.base_style = style;
    *element.interactivity() = caller;
    element
}

fn blank(
    size: Pixels,
    tall: Pixels,
    rounded: Pixels,
    muted: Hsla,
    glyph: Hsla,
    fallback: SharedString,
    text: Option<SharedString>,
) -> Div {
    let cover = match text {
        Some(text) => div()
            .text_size(size.min(tall) * 0.26)
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(glyph)
            .child(text)
            .into_any_element(),
        None => svg()
            .path(icons::path(fallback))
            .size(size.min(tall) * 0.46)
            .text_color(glyph)
            .into_any_element(),
    };
    div()
        .w(size)
        .h(tall)
        .rounded(rounded)
        .bg(muted.opacity(0.12))
        .flex()
        .items_center()
        .justify_center()
        .child(cover)
}

/// The edge a spinning cover is baked at: the size it is drawn at.
const SPIN_EDGE: u32 = 192;
/// How many frames one whole turn is cut into: one for every frame a 60 Hz display draws
/// during the playing cover's whole turn, so a slow disc advances a degree at a time rather
/// than stepping in threes. A faster display shows each frame more than once.
const TURN_FRAMES: usize = 360;
/// How far ahead of the playhead frames are baked, and how far behind them they are kept.
/// A cover's whole turn is 44 MB of frames, so a strip holds only about a second of turn
/// around the playhead, and the frames behind it are baked again when the turn comes back
/// round.
const TURN_AHEAD: usize = 30;
const TURN_BEHIND: usize = 24;
/// How many poses a one-shot roll is cut into. The strip's thread bakes them ahead of the
/// roll, so a roll never turns a frame on the drawing thread; a 560 ms roll shows a pose
/// for every frame of a 60 Hz display.
const ROLL_POSES: usize = 30;
/// The step through the turn between two roll poses.
const ROLL_STRIDE: usize = TURN_FRAMES / ROLL_POSES;
/// How many turns are kept: the playing cover, the one rolling off it, and the turns still
/// held between rolls. Only those two spin, so a stack of mostly still covers costs no more
/// than they do.
const TURN_STRIPS: usize = 6;
/// How long a turning thread waits on its playhead before it looks again at whether its
/// cover is still there.
const TURN_PATIENCE: Duration = Duration::from_millis(250);

thread_local! {
    /// The turns behind spinning covers, the most recently drawn first. Each strip bakes
    /// its frames on its own thread just ahead of the playhead, so drawing a frame costs a
    /// lookup rather than a resample in the middle of the frame.
    static TURNING: RefCell<Vec<((Resource, u32), TurnStrip)>> = const { RefCell::new(Vec::new()) };
}

/// One cover's turn: the square it turns and the frames drawn out of it, shared with the
/// thread that bakes them. The thread holds a `Weak`, so letting the strip go stops it once
/// it next looks.
struct TurnFrames {
    still: Arc<RenderImage>,
    edge: u32,
    /// The square the turn is sampled from, made once on the turning thread.
    base: OnceLock<Option<RgbaImage>>,
    /// Where in `0..TURN_FRAMES` the drawn frame of the playing turn is.
    playhead: AtomicUsize,
    /// Whether the playing cover is turning, so the thread bakes its frames ahead of the
    /// roll poses rather than beside them.
    playing: AtomicBool,
    /// Whether a roll pose has been asked for. Poses are baked only for a cover rolling off
    /// the stack, never ahead of time for every cover on it.
    rolling: AtomicBool,
    slots: Mutex<Vec<Option<Arc<RenderImage>>>>,
    /// The poses a roll draws, a coarser turn that is kept whole so a roll always finds one.
    roll: Mutex<Vec<Option<Arc<RenderImage>>>>,
    /// Wakes the turning thread when the playhead moves.
    wake: (Mutex<()>, Condvar),
}

impl TurnFrames {
    fn new(still: Arc<RenderImage>, edge: u32) -> Self {
        Self {
            still,
            edge,
            base: OnceLock::new(),
            playhead: AtomicUsize::new(0),
            playing: AtomicBool::new(false),
            rolling: AtomicBool::new(false),
            slots: Mutex::new(vec![None; TURN_FRAMES]),
            roll: Mutex::new(vec![None; ROLL_POSES]),
            wake: (Mutex::new(()), Condvar::new()),
        }
    }

    /// Turns the still into the square the turn samples, once. It runs on the turning
    /// thread, so a cover arriving on the strip never resamples in the middle of a frame.
    fn prepare(&self) {
        let _ = self.base.set(turned_base(&self.still, self.edge));
    }

    /// The square the turn samples, once `prepare` has made it.
    fn base(&self) -> Option<&RgbaImage> {
        self.base.get().and_then(|base| base.as_ref())
    }

    /// The frame at `index`, turned now from `base`.
    fn bake(&self, base: &RgbaImage, index: usize) -> Arc<RenderImage> {
        let mut turned = match index {
            0 => base.clone(),
            // Counterclockwise, the way a cover rolls as it travels left.
            _ => rotate_cover(base, -std::f32::consts::TAU * index as f32 / TURN_FRAMES as f32),
        };
        bgra(&mut turned);
        Arc::new(RenderImage::new(vec![Frame::new(turned)]))
    }

    /// Bakes the first frame between the playhead and `TURN_AHEAD` past it that is still
    /// missing, and says whether there was one. The frame is turned without the lock held,
    /// so drawing keeps reading frames all the while.
    fn bake_ahead(&self, playhead: usize) -> bool {
        let Some(base) = self.base() else {
            return false;
        };
        let missing = {
            let slots = self
                .slots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (0..=TURN_AHEAD)
                .map(|step| (playhead + step) % TURN_FRAMES)
                .find(|index| slots[*index].is_none())
        };
        let Some(index) = missing else {
            return false;
        };
        let frame = self.bake(base, index);
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slots[index].is_none() {
            slots[index] = Some(frame);
        }
        true
    }

    /// Bakes the first pose of a roll that is still missing, and says whether there was
    /// one. Poses are turned in their own order, so a roll that starts before they are
    /// ready fills in behind the angle it is showing.
    fn bake_roll(&self) -> bool {
        let Some(base) = self.base() else {
            return false;
        };
        let missing = {
            let roll = self
                .roll
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (0..ROLL_POSES).find(|pose| roll[*pose].is_none())
        };
        let Some(pose) = missing else {
            return false;
        };
        let frame = self.bake(base, pose * ROLL_STRIDE);
        let mut roll = self
            .roll
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if roll[pose].is_none() {
            roll[pose] = Some(frame);
        }
        true
    }

    /// The frame nearest the playhead that is baked and not past it, from the playing
    /// window, or a roll pose at the same angle while the window is still being baked.
    fn frame(&self, playhead: usize) -> Option<Arc<RenderImage>> {
        let baked = {
            let slots = self
                .slots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (0..=TURN_BEHIND).find_map(|step| {
                let index = (playhead + TURN_FRAMES - step) % TURN_FRAMES;
                slots[index].clone()
            })
        };
        baked.or_else(|| self.roll_frame(playhead / ROLL_STRIDE))
    }

    /// The roll pose nearest `pose` that is baked and not past it.
    fn roll_frame(&self, pose: usize) -> Option<Arc<RenderImage>> {
        let roll = self
            .roll
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (0..ROLL_POSES).find_map(|step| roll[(pose + ROLL_POSES - step) % ROLL_POSES].clone())
    }

    /// Forgets the frames further from the playhead than the window keeps, handing each to
    /// `release` so the sprite atlas lets go of it too.
    fn sweep(&self, playhead: usize, release: &mut impl FnMut(Arc<RenderImage>)) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (index, slot) in slots.iter_mut().enumerate() {
            let ahead = (index + TURN_FRAMES - playhead) % TURN_FRAMES;
            let behind = (playhead + TURN_FRAMES - index) % TURN_FRAMES;
            if ahead > TURN_AHEAD
                && behind > TURN_BEHIND
                && let Some(frame) = slot.take()
            {
                release(frame);
            }
        }
    }

    /// Hands every baked frame to `release`, for a strip leaving the cache.
    fn forget(&self, release: &mut impl FnMut(Arc<RenderImage>)) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for slot in slots.iter_mut() {
            if let Some(frame) = slot.take() {
                release(frame);
            }
        }
        drop(slots);
        let mut roll = self
            .roll
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for pose in roll.iter_mut() {
            if let Some(frame) = pose.take() {
                release(frame);
            }
        }
    }
}

/// One cover's turn and the frames drawn from it. `new` holds the still; the turning thread
/// makes the square it samples and the frames as the playhead reaches them.
struct TurnStrip {
    frames: Arc<TurnFrames>,
    /// Whether the turning thread has been started for this strip.
    turning: bool,
}

impl TurnStrip {
    fn new(still: Arc<RenderImage>, edge: u32) -> Self {
        Self {
            frames: Arc::new(TurnFrames::new(still, edge)),
            turning: false,
        }
    }

    /// Moves the playing turn's playhead to `index`, or draws roll pose `roll`, and gives the
    /// frame to draw now: the nearest baked frame at or behind that angle, with the turning
    /// thread set to baking what is missing. Drawing never turns a frame itself. `release`
    /// receives the frames that fell out of the window.
    fn frame(
        &mut self,
        index: usize,
        roll: Option<usize>,
        release: &mut impl FnMut(Arc<RenderImage>),
    ) -> Option<Arc<RenderImage>> {
        self.start_turning();
        match roll {
            Some(pose) => {
                self.frames.playing.store(false, Ordering::Relaxed);
                self.frames.rolling.store(true, Ordering::Relaxed);
                self.frames.wake.1.notify_one();
                self.frames.roll_frame(pose)
            }
            None => {
                self.frames.playing.store(true, Ordering::Relaxed);
                self.frames.playhead.store(index, Ordering::Relaxed);
                self.frames.wake.1.notify_one();
                self.frames.sweep(index, release);
                self.frames.frame(index)
            }
        }
    }

    /// Starts the strip's turning thread on first use.
    fn start_turning(&mut self) {
        if self.turning {
            return;
        }
        self.turning = true;
        let weak = Arc::downgrade(&self.frames);
        let started = std::thread::Builder::new()
            .name("turning".into())
            .spawn(move || turn(weak));
        if let Err(error) = started {
            log::warn!("artwork: cannot start a turning thread: {error}");
        }
    }

    /// Hands every baked frame to `release`, for a strip leaving the cache.
    fn forget(&self, release: &mut impl FnMut(Arc<RenderImage>)) {
        self.frames.forget(release);
    }
}

/// Bakes `turn`'s frames as they are drawn: the playing window ahead of the playhead first,
/// then the poses of a roll that has asked for them, and returns once the strip is gone.
fn turn(weak: Weak<TurnFrames>) {
    loop {
        let Some(frames) = weak.upgrade() else {
            return;
        };
        if frames.base.get().is_none() {
            frames.prepare();
            continue;
        }
        let rolling = frames.rolling.load(Ordering::Relaxed);
        let baked = match frames.playing.load(Ordering::Relaxed) {
            true => {
                let playhead = frames.playhead.load(Ordering::Relaxed);
                frames.bake_ahead(playhead) || (rolling && frames.bake_roll())
            }
            // A roll draws its own poses, so nothing of the playing window is on screen.
            false => rolling && frames.bake_roll(),
        };
        if baked {
            continue;
        }
        let (lock, wake) = &frames.wake;
        let guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = wake.wait_timeout(guard, TURN_PATIENCE);
        // The strip may have gone while waiting for the playhead to move.
        drop(frames);
    }
}

/// The frame of the turn that `turn`, from 0 to 1, falls on. A whole turn lands in the
/// frame it started from.
fn turn_index(turn: f32) -> usize {
    (turn.clamp(0., 1.) * TURN_FRAMES as f32).round() as usize % TURN_FRAMES
}

/// The roll pose that `turn`, from 0 to 1, falls on, out of the coarser set a roll draws.
fn roll_index(turn: f32) -> usize {
    (turn.clamp(0., 1.) * ROLL_POSES as f32).round() as usize % ROLL_POSES
}

/// The frame of `resource`'s turn that `turn` falls on, baked ahead by the strip's own
/// thread, or the newest frame behind it while the frames around it are still being baked.
/// An `eager` turn is a roll, drawn from the coarse poses the thread bakes for rolls.
fn turned_frame(
    resource: &Resource,
    still: &Arc<RenderImage>,
    edge: u32,
    turn: f32,
    eager: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let index = turn_index(turn);
    let roll = match eager {
        true => Some(roll_index(turn)),
        false => None,
    };
    TURNING.with(|cache| {
        let mut cache = cache.borrow_mut();
        let key = (resource.clone(), edge);
        let at = match cache.iter().position(|(held, _)| held == &key) {
            Some(at) => at,
            None => {
                cache.insert(0, (key, TurnStrip::new(still.clone(), edge)));
                if cache.len() > TURN_STRIPS {
                    for (_, strip) in cache.drain(TURN_STRIPS..) {
                        strip.forget(&mut |frame| cx.drop_image(frame, Some(window)));
                    }
                }
                0
            }
        };
        // Most recently drawn first, so a strip between turns is what an arriving cover
        // evicts.
        if at != 0 {
            let drawn = cache.remove(at);
            cache.insert(0, drawn);
        }
        let (_, strip) = &mut cache[0];
        let mut dropped = Vec::new();
        let frame = strip.frame(index, roll, &mut |frame| dropped.push(frame));
        for frame in dropped {
            cx.drop_image(frame, Some(window));
        }
        frame
    })
}

/// A cover drawn from frames turned around its centre. `turn` is how far through one whole
/// counterclockwise turn it is, from 0 to 1; the caller drives it from the slide's own clock,
/// so the cover turns exactly as it travels and lands upright.
#[derive(IntoElement)]
pub struct SpinningArtwork {
    url: Option<SharedString>,
    size: Pixels,
    turn: f32,
    eager: bool,
}

impl SpinningArtwork {
    #[track_caller]
    pub fn new(url: Option<impl Into<SharedString>>) -> Self {
        Self {
            url: url.map(Into::into),
            size: px(28.),
            turn: 0.,
            eager: false,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// How far through its whole turn the cover is: 0 upright, 1 upright again.
    pub fn turn(mut self, turn: f32) -> Self {
        self.turn = turn;
        self
    }

    /// Draws a one-shot roll: the frame comes from the coarse poses the strip bakes for
    /// rolls, which the thread keeps up with, so a whole turn never lands as work on the
    /// drawing thread.
    pub fn eager(mut self) -> Self {
        self.eager = true;
        self
    }
}

impl RenderOnce for SpinningArtwork {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = snapped(self.size, window);
        let fallback = Artwork::new(self.url.clone())
            .size(size)
            .circle()
            .into_any_element();
        let Some(url) = self.url else {
            return fallback;
        };

        let cache = ArtworkCache::entity(cx);
        let resource = resource(url);
        let edge = sample_edge(size, window);
        let loaded = cache.update(cx, |cache, cx| cache.load_at(&resource, edge, window, cx));
        let turned = match loaded {
            Some(Ok(still)) => turned_frame(
                &resource,
                &still,
                edge.min(SPIN_EDGE),
                self.turn,
                self.eager,
                window,
                cx,
            ),
            Some(Err(_)) | None => None,
        };
        match turned {
            Some(frame) => img(ImageSource::Render(frame))
                .size(size)
                .object_fit(ObjectFit::Cover)
                .rounded(size / 2.)
                .into_any_element(),
            None => fallback,
        }
    }
}

/// Crops a still to its centre square and scales it to `edge`, the square the turn samples.
/// The still arrives BGRA; the turn is sampled in RGBA.
fn turned_base(still: &RenderImage, edge: u32) -> Option<RgbaImage> {
    let bytes = still.as_bytes(0)?;
    let size = still.size(0);
    let (width, height) = (size.width.0 as u32, size.height.0 as u32);
    if width == 0 || height == 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity(bytes.len());
    for pixel in bytes.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    let source = RgbaImage::from_raw(width, height, rgba)?;
    let side = width.min(height);
    let square =
        imageops::crop_imm(&source, (width - side) / 2, (height - side) / 2, side, side).to_image();
    Some(match side > edge {
        true => imageops::resize(&square, edge, edge, imageops::FilterType::Triangle),
        false => square,
    })
}

fn rotate_cover(source: &RgbaImage, angle: f32) -> RgbaImage {
    let (width, height) = source.dimensions();
    let center = (width as f32 - 1.) / 2.;
    let (sin, cos) = angle.sin_cos();
    let mut turned = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let sx = cos * dx + sin * dy + center;
            let sy = -sin * dx + cos * dy + center;
            let pixel = sample_cover(source, sx, sy);
            turned.put_pixel(x, y, pixel);
        }
    }
    turned
}

fn sample_cover(source: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (width, height) = source.dimensions();
    if x < 0. || y < 0. || x > (width - 1) as f32 || y > (height - 1) as f32 {
        return Rgba([0, 0, 0, 0]);
    }
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let sample = |x: u32, y: u32, weight: f32| {
        let pixel = source.get_pixel(x, y);
        [
            pixel[0] as f32 * weight,
            pixel[1] as f32 * weight,
            pixel[2] as f32 * weight,
            pixel[3] as f32 * weight,
        ]
    };
    let mut out = [0.; 4];
    for (x, y, weight) in [
        (x0, y0, (1. - fx) * (1. - fy)),
        (x1, y0, fx * (1. - fy)),
        (x0, y1, (1. - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        let pixel = sample(x, y, weight);
        for channel in 0..4 {
            out[channel] += pixel[channel];
        }
    }
    Rgba([
        out[0].round().clamp(0., 255.) as u8,
        out[1].round().clamp(0., 255.) as u8,
        out[2].round().clamp(0., 255.) as u8,
        out[3].round().clamp(0., 255.) as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Rgba, codecs::gif::GifEncoder};

    #[test]
    fn a_turning_cover_bakes_a_whole_turn() {
        let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::from_pixel(
            SPIN_EDGE,
            SPIN_EDGE,
            Rgba([40, 80, 120, 255]),
        ))]));
        let strip = TurnStrip::new(image.clone(), SPIN_EDGE);
        let base = turned_base(&image, SPIN_EDGE).expect("base");
        let start = strip.frames.bake(&base, 0);
        let quarter = strip.frames.bake(&base, TURN_FRAMES / 4);
        assert_eq!(start.size(0), image.size(0));
        assert!(!Arc::ptr_eq(&start, &quarter));
        // One whole turn lands in the pose it started in.
        assert_eq!(turn_index(1.), turn_index(0.));
        assert_eq!(turn_index(0.25), TURN_FRAMES / 4);
    }

    #[test]
    fn artwork_loader_targets_the_requested_edge() {
        let image = RgbaImage::from_pixel(240, 120, Rgba([20, 40, 60, 255]));
        let frame = artwork_frame(DynamicImage::ImageRgba8(image), 64);

        assert_eq!(frame.width(), 64);
        assert_eq!(frame.height(), 64);
    }

    #[test]
    fn artwork_loader_preserves_animated_frames() {
        let mut bytes = Vec::new();
        GifEncoder::new(&mut bytes)
            .encode_frames([
                Frame::from_parts(
                    RgbaImage::from_pixel(120, 120, Rgba([20, 40, 60, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(80, 1),
                ),
                Frame::from_parts(
                    RgbaImage::from_pixel(120, 120, Rgba([80, 100, 120, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(120, 1),
                ),
            ])
            .unwrap();

        let frames = raster_frames(&bytes, ImageFormat::Gif, 64).unwrap();

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].buffer().dimensions(), (64, 64));
        assert_eq!(frames[0].delay(), Delay::from_numer_denom_ms(80, 1));
        assert_eq!(frames[1].delay(), Delay::from_numer_denom_ms(120, 1));
    }
}
