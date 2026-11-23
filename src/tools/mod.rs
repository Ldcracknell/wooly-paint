use crate::document::composite::div255;
use crate::document::Layer;
use crate::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Brush,
    Pixel,
    Eraser,
    Eyedropper,
    Fill,
    Line,
    Rect,
    Ellipse,
    SelectRect,
    MagicSelect,
    Move,
    Hand,
}

impl ToolKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Brush => "Brush",
            Self::Pixel => "Pixel",
            Self::Eraser => "Eraser",
            Self::Eyedropper => "Eyedropper",
            Self::Fill => "Fill",
            Self::Line => "Line",
            Self::Rect => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::SelectRect => "Select",
            Self::MagicSelect => "Magic select",
            Self::Move => "Move",
            Self::Hand => "Hand",
        }
    }

    pub fn dropdown_index(self) -> u32 {
        match self {
            Self::Brush => 0,
            Self::Pixel => 1,
            Self::Eraser => 2,
            Self::Eyedropper => 3,
            Self::Fill => 4,
            Self::Line => 5,
            Self::Rect => 6,
            Self::Ellipse => 7,
            Self::SelectRect => 8,
            Self::MagicSelect => 9,
            Self::Move => 10,
            Self::Hand => 11,
        }
    }
}

#[inline(always)]
fn straight_to_premul_rgba(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    if a == 255 {
        return [r, g, b, 255];
    }
    if a == 0 {
        return [0, 0, 0, 0];
    }
    let a32 = a as u32;
    [
        div255(r as u32 * a32) as u8,
        div255(g as u32 * a32) as u8,
        div255(b as u32 * a32) as u8,
        a,
    ]
}

pub type DirtyRect = (i32, i32, i32, i32);

fn union_dirty(a: Option<DirtyRect>, b: Option<DirtyRect>) -> Option<DirtyRect> {
    match (a, b) {
        (None, r) | (r, None) => r,
        (Some((ax, ay, aw, ah)), Some((bx, by, bw, bh))) => {
            let x0 = ax.min(bx);
            let y0 = ay.min(by);
            let x1 = (ax + aw).max(bx + bw);
            let y1 = (ay + ah).max(by + bh);
            Some((x0, y0, x1 - x0, y1 - y0))
        }
    }
}

fn clip_dirty_to_layer(layer: &Layer, x: i32, y: i32, w: i32, h: i32) -> Option<DirtyRect> {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(layer.width as i32);
    let y1 = (y + h).min(layer.height as i32);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

/// Precomputed integer loop bounds + optional region mask pointer so `Selection::Rect` clipping
/// costs zero branches per pixel and `Selection::Region` uses a direct slice index.
struct ClipLoopBounds<'a> {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    mask: Option<(&'a [u8], usize)>,
}

fn clip_loop_bounds<'a>(
    layer_w: i32,
    layer_h: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    clip: Option<&'a Selection>,
) -> Option<ClipLoopBounds<'a>> {
    let mut bx0 = x0.max(0);
    let mut by0 = y0.max(0);
    let mut bx1 = x1.min(layer_w);
    let mut by1 = y1.min(layer_h);
    let mut mask = None;

    if let Some(sel) = clip {
        match sel {
            Selection::Rect(sx, sy, sw, sh) => {
                bx0 = bx0.max(*sx);
                by0 = by0.max(*sy);
                bx1 = bx1.min(sx.saturating_add(*sw));
                by1 = by1.min(sy.saturating_add(*sh));
            }
            Selection::Region {
                width,
                height,
                mask: m,
                tight_bbox,
                ..
            } => {
                let rw = *width as i32;
                let rh = *height as i32;
                bx1 = bx1.min(rw);
                by1 = by1.min(rh);
                if let Some((tx, ty, tw, th)) = *tight_bbox {
                    bx0 = bx0.max(tx);
                    by0 = by0.max(ty);
                    bx1 = bx1.min(tx.saturating_add(tw));
                    by1 = by1.min(ty.saturating_add(th));
                }
                if m.len() == (*width as usize) * (*height as usize) {
                    mask = Some((m.as_slice(), *width as usize));
                } else {
                    return None;
                }
            }
        }
    }

    if bx1 <= bx0 || by1 <= by0 {
        None
    } else {
        Some(ClipLoopBounds {
            x0: bx0,
            y0: by0,
            x1: bx1,
            y1: by1,
            mask,
        })
    }
}

#[inline(always)]
fn clip_allows_point(clip: Option<&Selection>, ix: i32, iy: i32) -> bool {
    match clip {
        None => true,
        Some(Selection::Rect(sx, sy, sw, sh)) => {
            ix >= *sx && iy >= *sy && ix < sx.saturating_add(*sw) && iy < sy.saturating_add(*sh)
        }
        Some(Selection::Region {
            width,
            height,
            mask,
            ..
        }) => {
            if ix < 0 || iy < 0 || ix >= *width as i32 || iy >= *height as i32 {
                return false;
            }
            mask.get((iy as u32 * width + ix as u32) as usize)
                .copied()
                .unwrap_or(0)
                != 0
        }
    }
}

#[inline(always)]
fn blend_premul_pixel_int(dst: &mut [u8], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        dst[0] = src[0];
        dst[1] = src[1];
        dst[2] = src[2];
        dst[3] = 255;
        return;
    }
    let inv = 255 - sa;
    dst[0] = (src[0] as u32 + div255(dst[0] as u32 * inv)).min(255) as u8;
    dst[1] = (src[1] as u32 + div255(dst[1] as u32 * inv)).min(255) as u8;
    dst[2] = (src[2] as u32 + div255(dst[2] as u32 * inv)).min(255) as u8;
    dst[3] = (sa + div255(dst[3] as u32 * inv)).min(255) as u8;
}

#[inline(always)]
fn apply_brush_pixel(dst: &mut [u8], color: [u8; 4], mask_a: u8, eraser: bool) {
    if mask_a == 0 {
        return;
    }
    let color_a = color[3] as u32;
    let eff_a = if color_a == 255 {
        mask_a as u32
    } else {
        div255(mask_a as u32 * color_a)
    };
    if eff_a == 0 {
        return;
    }
    if eraser {
        if eff_a == 255 {
            dst[0] = 0;
            dst[1] = 0;
            dst[2] = 0;
            dst[3] = 0;
        } else {
            let inv = 255 - eff_a;
            dst[0] = div255(dst[0] as u32 * inv) as u8;
            dst[1] = div255(dst[1] as u32 * inv) as u8;
            dst[2] = div255(dst[2] as u32 * inv) as u8;
            dst[3] = div255(dst[3] as u32 * inv) as u8;
        }
    } else if eff_a == 255 {
        dst[0] = color[0];
        dst[1] = color[1];
        dst[2] = color[2];
        dst[3] = 255;
    } else {
        let src = [
            div255(color[0] as u32 * eff_a) as u8,
            div255(color[1] as u32 * eff_a) as u8,
            div255(color[2] as u32 * eff_a) as u8,
            eff_a as u8,
        ];
        blend_premul_pixel_int(dst, src);
    }
}

/// Falloff LUT for a fixed `(radius, hardness)` where `hardness = 0.0` is softest airbrush
/// and `hardness = 1.0` is a solid hard brush with 1px smooth anti-aliasing at the rim.
struct BrushFalloffCache {
    lut: [u8; 256],
}

impl BrushFalloffCache {
    fn new(radius: f64, hardness: f64) -> Self {
        let hard = hardness.clamp(0.0, 1.0);
        let r = radius.max(0.55);
        // Normalized rim fade width in `u = 1 - d/r` units:
        // at hard=1.0 -> 1px anti-aliased edge (`min(1.0 / r, 1.0)`),
        // at hard=0.0 -> full radius feathering (`1.0`).
        let aa_u = (1.0 / r).min(1.0);
        let fade_u = (1.0 - hard).max(aa_u);
        let power = 1.0 + 1.2 * (1.0 - hard);
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let u = i as f64 / 255.0;
            let a_straight = if u <= 0.0 {
                0.0
            } else if u >= fade_u {
                1.0
            } else {
                (u / fade_u).powf(power).clamp(0.0, 1.0)
            };
            *slot = (a_straight * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        Self { lut }
    }

    #[inline(always)]
    fn sample_u8(&self, u: f64) -> u8 {
        if u <= 0.0 {
            return 0;
        }
        if u >= 1.0 {
            return self.lut[255];
        }
        let ui = (u * 255.0 + 0.5) as usize;
        self.lut[ui.min(255)]
    }
}

const BRUSH_SUBPIXEL_STEPS: usize = 4;
const BRUSH_SUBPIXEL_MASKS: usize = BRUSH_SUBPIXEL_STEPS * BRUSH_SUBPIXEL_STEPS;

struct BrushStampMask {
    rel_x0: i32,
    rel_y0: i32,
    width: i32,
    height: i32,
    /// Pure geometric mask alpha `0..=255` (independent of paint color alpha).
    alpha: Vec<u8>,
}

impl BrushStampMask {
    fn new(radius: f64, falloff: &BrushFalloffCache, qx: usize, qy: usize) -> Self {
        // Minimum 0.55 so a 1px brush (radius 0.5) always covers the pixel under any subpixel center.
        let r = radius.max(0.55);
        let r2 = r * r;
        let inv_r = 1.0 / r;
        let cx = (qx as f64 + 0.5) / BRUSH_SUBPIXEL_STEPS as f64;
        let cy = (qy as f64 + 0.5) / BRUSH_SUBPIXEL_STEPS as f64;
        let x0 = (cx - r - 1.0).floor() as i32;
        let y0 = (cy - r - 1.0).floor() as i32;
        let x1 = (cx + r + 1.0).ceil() as i32;
        let y1 = (cy + r + 1.0).ceil() as i32;
        let width = (x1 - x0).max(0);
        let height = (y1 - y0).max(0);
        let mut alpha = vec![0u8; (width * height) as usize];

        for row in 0..height {
            let iy = y0 + row;
            let dy = iy as f64 + 0.5 - cy;
            let dy2 = dy * dy;
            if dy2 > r2 {
                continue;
            }
            let max_dx = (r2 - dy2).sqrt();
            let col_start = ((cx - max_dx - 0.5).floor() as i32 - x0).clamp(0, width);
            let col_end = ((cx + max_dx - 0.5).ceil() as i32 + 1 - x0).clamp(0, width);
            let row_base = (row * width) as usize;
            for col in col_start..col_end {
                let ix = x0 + col;
                let dx = ix as f64 + 0.5 - cx;
                let d2 = dx * dx + dy2;
                if d2 > r2 {
                    continue;
                }
                let u = (1.0 - d2.sqrt() * inv_r).clamp(0.0, 1.0);
                alpha[row_base + col as usize] = falloff.sample_u8(u);
            }
        }

        Self {
            rel_x0: x0,
            rel_y0: y0,
            width,
            height,
            alpha,
        }
    }
}

/// Reusable brush falloff + quantized subpixel mask cache (persisted across drag events in `AppState`).
pub struct BrushStampCache {
    radius_bits: u64,
    hardness_bits: u64,
    radius: f64,
    falloff: BrushFalloffCache,
    masks: [Option<BrushStampMask>; BRUSH_SUBPIXEL_MASKS],
}

impl BrushStampCache {
    pub fn new() -> Self {
        Self {
            radius_bits: f64::NAN.to_bits(),
            hardness_bits: f64::NAN.to_bits(),
            radius: 0.55,
            falloff: BrushFalloffCache::new(0.55, 0.85),
            masks: std::array::from_fn(|_| None),
        }
    }

    pub fn prepare(&mut self, radius: f64, hardness: f64) {
        let rb = radius.to_bits();
        let hb = hardness.to_bits();
        if self.radius_bits == rb && self.hardness_bits == hb {
            return;
        }
        self.radius_bits = rb;
        self.hardness_bits = hb;
        self.radius = radius;
        self.falloff = BrushFalloffCache::new(radius, hardness);
        for slot in &mut self.masks {
            *slot = None;
        }
    }

    #[inline(always)]
    fn quantized_index(cx: f64, cy: f64) -> (usize, usize, usize) {
        let fx = (cx - cx.floor()).clamp(0.0, 0.999_999);
        let fy = (cy - cy.floor()).clamp(0.0, 0.999_999);
        let qx = (fx * BRUSH_SUBPIXEL_STEPS as f64) as usize;
        let qy = (fy * BRUSH_SUBPIXEL_STEPS as f64) as usize;
        (qy * BRUSH_SUBPIXEL_STEPS + qx, qx, qy)
    }

    fn mask(&mut self, cx: f64, cy: f64) -> &BrushStampMask {
        let (idx, qx, qy) = Self::quantized_index(cx, cy);
        if self.masks[idx].is_none() {
            self.masks[idx] = Some(BrushStampMask::new(self.radius, &self.falloff, qx, qy));
        }
        self.masks[idx].as_ref().expect("brush mask inserted")
    }
}

impl Default for BrushStampCache {
    fn default() -> Self {
        Self::new()
    }
}

fn stamp_circle_with_falloff(
    layer: &mut Layer,
    cx: f64,
    cy: f64,
    radius: f64,
    color: [u8; 4],
    eraser: bool,
    cache: &mut BrushStampCache,
    clip: Option<&Selection>,
) -> Option<DirtyRect> {
    if radius <= 0.0 || color[3] == 0 {
        return None;
    }
    let w = layer.width as i32;
    let h = layer.height as i32;
    let mask = cache.mask(cx, cy);
    let x0 = cx.floor() as i32 + mask.rel_x0;
    let y0 = cy.floor() as i32 + mask.rel_y0;
    let x1 = x0 + mask.width;
    let y1 = y0 + mask.height;

    let bounds = clip_loop_bounds(w, h, x0, y0, x1, y1, clip)?;
    let layer_stride = layer.width as usize * 4;
    let mask_w = mask.width as usize;

    if let Some((clip_mask, clip_w)) = bounds.mask {
        for iy in bounds.y0..bounds.y1 {
            let mask_row = (iy - y0) as usize * mask_w;
            let clip_row = iy as usize * clip_w;
            let pix_row = iy as usize * layer_stride;
            for ix in bounds.x0..bounds.x1 {
                let aq = mask.alpha[mask_row + (ix - x0) as usize];
                if aq == 0 || clip_mask[clip_row + ix as usize] == 0 {
                    continue;
                }
                let i = pix_row + ix as usize * 4;
                apply_brush_pixel(&mut layer.pixels[i..i + 4], color, aq, eraser);
            }
        }
    } else {
        for iy in bounds.y0..bounds.y1 {
            let mask_row = (iy - y0) as usize * mask_w;
            let pix_row = iy as usize * layer_stride;
            for ix in bounds.x0..bounds.x1 {
                let aq = mask.alpha[mask_row + (ix - x0) as usize];
                if aq == 0 {
                    continue;
                }
                let i = pix_row + ix as usize * 4;
                apply_brush_pixel(&mut layer.pixels[i..i + 4], color, aq, eraser);
            }
        }
    }
    Some((
        bounds.x0,
        bounds.y0,
        bounds.x1 - bounds.x0,
        bounds.y1 - bounds.y0,
    ))
}

/// `color` straight RGBA; composites with existing premultiplied canvas.
pub fn stamp_circle_cached(
    layer: &mut Layer,
    cx: f64,
    cy: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
    cache: &mut BrushStampCache,
) -> Option<DirtyRect> {
    if radius <= 0.0 {
        return None;
    }
    cache.prepare(radius, hardness);
    stamp_circle_with_falloff(layer, cx, cy, radius, color, eraser, cache, clip)
}

fn brush_spacing(radius: f64, hardness: f64) -> f64 {
    let r = radius.max(0.55);
    let h = hardness.clamp(0.0, 1.0);
    // Keep chord sagitta < 0.25 px (`step <= sqrt(2 * r)`) so hard strokes never scallop while
    // large brushes stay fast (e.g. ~12px spacing at size=256 instead of sub-pixel dabs).
    let sagitta_bound = (2.0 * r).sqrt();
    let frac = 0.14 + 0.08 * h;
    (r * frac).min(sagitta_bound).max(0.35)
}

fn brush_stroke_step(radius: f64, hardness: f64) -> f64 {
    brush_spacing(radius, hardness)
}

/// Single-pass capsule SDF line rasterizer for the Line tool: exact uniform alpha even when
/// semi-transparent and O(area) regardless of line length.
pub fn draw_line_capsule(
    layer: &mut Layer,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
    cache: &mut BrushStampCache,
) -> Option<DirtyRect> {
    if radius <= 0.0 || color[3] == 0 {
        return None;
    }
    cache.prepare(radius, hardness);
    let r = radius.max(0.55);
    let r2 = r * r;
    let inv_r = 1.0 / r;

    let min_x = (x0.min(x1) - r - 1.0).floor() as i32;
    let min_y = (y0.min(y1) - r - 1.0).floor() as i32;
    let max_x = (x0.max(x1) + r + 1.0).ceil() as i32;
    let max_y = (y0.max(y1) + r + 1.0).ceil() as i32;

    let bounds = clip_loop_bounds(
        layer.width as i32,
        layer.height as i32,
        min_x,
        min_y,
        max_x,
        max_y,
        clip,
    )?;

    let vx = x1 - x0;
    let vy = y1 - y0;
    let seg_len2 = vx * vx + vy * vy;
    let inv_seg_len2 = if seg_len2 > 1e-12 {
        1.0 / seg_len2
    } else {
        0.0
    };
    let layer_stride = layer.width as usize * 4;
    let falloff = &cache.falloff;

    for iy in bounds.y0..bounds.y1 {
        let py = iy as f64 + 0.5;
        let apy = py - y0;
        let pix_row = iy as usize * layer_stride;
        for ix in bounds.x0..bounds.x1 {
            if let Some((clip_mask, clip_w)) = bounds.mask {
                if clip_mask[iy as usize * clip_w + ix as usize] == 0 {
                    continue;
                }
            }
            let px = ix as f64 + 0.5;
            let apx = px - x0;
            let t = if inv_seg_len2 > 0.0 {
                ((apx * vx + apy * vy) * inv_seg_len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let dx = apx - vx * t;
            let dy = apy - vy * t;
            let d2 = dx * dx + dy * dy;
            if d2 > r2 {
                continue;
            }
            let u = (1.0 - d2.sqrt() * inv_r).clamp(0.0, 1.0);
            let aq = falloff.sample_u8(u);
            if aq == 0 {
                continue;
            }
            let i = pix_row + ix as usize * 4;
            apply_brush_pixel(&mut layer.pixels[i..i + 4], color, aq, eraser);
        }
    }

    Some((
        bounds.x0,
        bounds.y0,
        bounds.x1 - bounds.x0,
        bounds.y1 - bounds.y0,
    ))
}

pub fn stroke_line_spaced_cached(
    layer: &mut Layer,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
    next_dab_distance: &mut f64,
    cache: &mut BrushStampCache,
) -> Option<DirtyRect> {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len <= 1e-9 {
        return None;
    }

    let spacing = brush_spacing(radius, hardness);
    if !next_dab_distance.is_finite() || *next_dab_distance <= 0.0 {
        *next_dab_distance = spacing;
    }

    cache.prepare(radius, hardness);
    let mut dirty = None;
    let mut d = *next_dab_distance;
    while d <= len {
        let t = d / len;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        dirty = union_dirty(
            dirty,
            stamp_circle_with_falloff(layer, x, y, radius, color, eraser, cache, clip),
        );
        d += spacing;
    }
    *next_dab_distance = d - len;
    dirty
}

pub fn stroke_quadratic_spaced_cached(
    layer: &mut Layer,
    x0: f64,
    y0: f64,
    cx: f64,
    cy: f64,
    x1: f64,
    y1: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
    next_dab_distance: &mut f64,
    cache: &mut BrushStampCache,
) -> Option<DirtyRect> {
    let chord = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    let control_net = ((cx - x0).powi(2) + (cy - y0).powi(2)).sqrt()
        + ((x1 - cx).powi(2) + (y1 - cy).powi(2)).sqrt();
    let approx_len = (chord + control_net) * 0.5;
    if !approx_len.is_finite() || approx_len <= 1e-9 {
        return None;
    }

    let spacing = brush_spacing(radius, hardness);
    if !next_dab_distance.is_finite() || *next_dab_distance <= 0.0 {
        *next_dab_distance = spacing;
    }

    let subdivisions = (approx_len / (spacing * 0.5)).ceil().clamp(4.0, 4096.0) as usize;
    cache.prepare(radius, hardness);
    let mut dirty = None;
    let mut remaining = *next_dab_distance;
    let mut px = x0;
    let mut py = y0;

    for i in 1..=subdivisions {
        let t = i as f64 / subdivisions as f64;
        let mt = 1.0 - t;
        let qx = mt * mt * x0 + 2.0 * mt * t * cx + t * t * x1;
        let qy = mt * mt * y0 + 2.0 * mt * t * cy + t * t * y1;
        let sx = qx - px;
        let sy = qy - py;
        let seg_len = (sx * sx + sy * sy).sqrt();
        if seg_len.is_finite() && seg_len > 1e-9 {
            let mut d = remaining;
            while d <= seg_len {
                let u = d / seg_len;
                dirty = union_dirty(
                    dirty,
                    stamp_circle_with_falloff(
                        layer,
                        px + sx * u,
                        py + sy * u,
                        radius,
                        color,
                        eraser,
                        cache,
                        clip,
                    ),
                );
                d += spacing;
            }
            remaining = d - seg_len;
        }
        px = qx;
        py = qy;
    }

    *next_dab_distance = remaining;
    dirty
}

pub fn stamp_square(
    layer: &mut Layer,
    cx: f64,
    cy: f64,
    size: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
) -> Option<DirtyRect> {
    if size <= 0.0 || color[3] == 0 {
        return None;
    }
    let s = (size.floor() as i32).max(1);
    let ax = cx.floor() as i32;
    let ay = cy.floor() as i32;
    let x0 = ax - (s - 1) / 2;
    let y0 = ay - (s - 1) / 2;
    let x1 = x0 + s;
    let y1 = y0 + s;

    let bounds = clip_loop_bounds(layer.width as i32, layer.height as i32, x0, y0, x1, y1, clip)?;
    let src = straight_to_premul_rgba(color[0], color[1], color[2], color[3]);
    let inv = 255u32 - color[3] as u32;
    let layer_stride = layer.width as usize * 4;

    if let Some((clip_mask, clip_w)) = bounds.mask {
        for iy in bounds.y0..bounds.y1 {
            let clip_row = iy as usize * clip_w;
            let pix_row = iy as usize * layer_stride;
            for ix in bounds.x0..bounds.x1 {
                if clip_mask[clip_row + ix as usize] == 0 {
                    continue;
                }
                let i = pix_row + ix as usize * 4;
                if eraser {
                    if inv == 0 {
                        layer.pixels[i..i + 4].fill(0);
                    } else {
                        for c in 0..4 {
                            layer.pixels[i + c] = div255(layer.pixels[i + c] as u32 * inv) as u8;
                        }
                    }
                } else {
                    blend_premul_pixel_int(&mut layer.pixels[i..i + 4], src);
                }
            }
        }
    } else {
        for iy in bounds.y0..bounds.y1 {
            let pix_row = iy as usize * layer_stride;
            for ix in bounds.x0..bounds.x1 {
                let i = pix_row + ix as usize * 4;
                if eraser {
                    if inv == 0 {
                        layer.pixels[i..i + 4].fill(0);
                    } else {
                        for c in 0..4 {
                            layer.pixels[i + c] = div255(layer.pixels[i + c] as u32 * inv) as u8;
                        }
                    }
                } else {
                    blend_premul_pixel_int(&mut layer.pixels[i..i + 4], src);
                }
            }
        }
    }
    Some((
        bounds.x0,
        bounds.y0,
        bounds.x1 - bounds.x0,
        bounds.y1 - bounds.y0,
    ))
}

fn bresenham_visit(mut x0: i32, mut y0: i32, x1: i32, y1: i32, mut visit: impl FnMut(i32, i32)) {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    loop {
        visit(x0, y0);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x0 += sx;
        }
        if e2 < dx {
            err += dx;
            y0 += sy;
        }
    }
}

pub fn stroke_line_square(
    layer: &mut Layer,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    size: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
) -> Option<DirtyRect> {
    let p0x = x0.floor() as i32;
    let p0y = y0.floor() as i32;
    let p1x = x1.floor() as i32;
    let p1y = y1.floor() as i32;
    let mut dirty = None;
    let skip_first = p0x != p1x || p0y != p1y;
    let mut first = true;
    bresenham_visit(p0x, p0y, p1x, p1y, |ix, iy| {
        if first && skip_first {
            first = false;
            return;
        }
        first = false;
        dirty = union_dirty(
            dirty,
            stamp_square(
                layer,
                ix as f64 + 0.5,
                iy as f64 + 0.5,
                size,
                color,
                eraser,
                clip,
            ),
        );
    });
    dirty
}

/// Connected pixels matching `layer[(x,y)]` within `tolerance` (premul RGBA). Does not modify the layer.
/// Uses a fast scanline fill (O(H) stack instead of 4× per-pixel pushes).
pub fn flood_select_mask(
    layer: &Layer,
    x: u32,
    y: u32,
    tolerance: u8,
) -> (Vec<u8>, Option<(i32, i32, i32, i32)>) {
    let w = layer.width as usize;
    let h = layer.height as usize;
    let mut mask = vec![0u8; w * h];
    if x >= layer.width || y >= layer.height {
        return (mask, None);
    }
    let start = layer.pixel_premul(x, y);
    let tol = tolerance as i32;
    let pixels = &layer.pixels;
    let matches_at = |idx: usize| -> bool {
        let p = idx * 4;
        (pixels[p] as i32 - start[0] as i32).abs() <= tol
            && (pixels[p + 1] as i32 - start[1] as i32).abs() <= tol
            && (pixels[p + 2] as i32 - start[2] as i32).abs() <= tol
            && (pixels[p + 3] as i32 - start[3] as i32).abs() <= tol
    };

    let mut min_x = w as i32;
    let mut min_y = h as i32;
    let mut max_x = -1i32;
    let mut max_y = -1i32;

    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(256);
    stack.push((x as usize, y as usize));

    while let Some((sx, sy)) = stack.pop() {
        let row = sy * w;
        if mask[row + sx] != 0 || !matches_at(row + sx) {
            continue;
        }
        let mut lx = sx;
        while lx > 0 && mask[row + lx - 1] == 0 && matches_at(row + lx - 1) {
            lx -= 1;
        }
        let mut rx = sx;
        while rx + 1 < w && mask[row + rx + 1] == 0 && matches_at(row + rx + 1) {
            rx += 1;
        }
        mask[row + lx..=row + rx].fill(1);
        min_x = min_x.min(lx as i32);
        max_x = max_x.max(rx as i32);
        min_y = min_y.min(sy as i32);
        max_y = max_y.max(sy as i32);

        for ny in [sy.checked_sub(1), (sy + 1 < h).then_some(sy + 1)]
            .into_iter()
            .flatten()
        {
            let nrow = ny * w;
            let mut in_span = false;
            for cx in lx..=rx {
                let ok = mask[nrow + cx] == 0 && matches_at(nrow + cx);
                if ok && !in_span {
                    stack.push((cx, ny));
                    in_span = true;
                } else if !ok {
                    in_span = false;
                }
            }
        }
    }

    let bbox = if max_x < min_x {
        None
    } else {
        Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    };
    (mask, bbox)
}

/// Uses `hint` when present (caller must guarantee it matches `mask`), else scans the mask.
pub fn region_tight_bbox_or_hint(
    mask: &[u8],
    width: u32,
    height: u32,
    hint: Option<(i32, i32, i32, i32)>,
) -> Option<(i32, i32, i32, i32)> {
    if let Some(b) = hint {
        return Some(b);
    }
    region_tight_bbox(mask, width, height)
}

/// Tight integer bounds `(x, y, w, h)` of all non-zero mask cells.
pub fn region_tight_bbox(mask: &[u8], width: u32, height: u32) -> Option<(i32, i32, i32, i32)> {
    let w = width as i32;
    let h = height as i32;
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = -1i32;
    let mut max_y = -1i32;
    for y in 0..height {
        let row = (y * width) as usize;
        for x in 0..width {
            if mask[row + x as usize] != 0 {
                let xi = x as i32;
                let yi = y as i32;
                min_x = min_x.min(xi);
                min_y = min_y.min(yi);
                max_x = max_x.max(xi);
                max_y = max_y.max(yi);
            }
        }
    }
    if max_x < min_x {
        return None;
    }
    Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

/// Copy premultiplied RGBA from `layer` into a `bw`×`bh` buffer; only pixels where `mask` is non-zero at doc coords.
pub fn copy_region_masked(
    layer: &Layer,
    mask: &[u8],
    bx: i32,
    by: i32,
    bw: i32,
    bh: i32,
) -> Vec<u8> {
    let w = layer.width;
    let h = layer.height;
    let mut out = vec![0u8; (bw * bh * 4) as usize];
    for row in 0..bh {
        for col in 0..bw {
            let sx = bx + col;
            let sy = by + row;
            let oi = ((row * bw + col) * 4) as usize;
            if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                let mi = (sy as u32 * w + sx as u32) as usize;
                if mi < mask.len() && mask[mi] != 0 {
                    let i = layer.idx(sx as u32, sy as u32);
                    out[oi..oi + 4].copy_from_slice(&layer.pixels[i..i + 4]);
                }
            }
        }
    }
    out
}

pub fn clear_region_masked_rect(
    layer: &mut Layer,
    mask: &[u8],
    bx: i32,
    by: i32,
    bw: i32,
    bh: i32,
) {
    let w = layer.width as usize;
    let h = layer.height as usize;
    let expected = w * h;
    if mask.len() != expected || bw <= 0 || bh <= 0 {
        return;
    }
    let x0 = bx.max(0) as usize;
    let y0 = by.max(0) as usize;
    let x1 = (bx + bw).min(layer.width as i32).max(0) as usize;
    let y1 = (by + bh).min(layer.height as i32).max(0) as usize;
    for y in y0..y1 {
        let row = y * w;
        for x in x0..x1 {
            let mi = row + x;
            if mask[mi] != 0 {
                let i = mi * 4;
                layer.pixels[i..i + 4].fill(0);
            }
        }
    }
}

/// Fast scanline flood fill respecting optional selection clip.
pub fn flood_fill(
    layer: &mut Layer,
    x: u32,
    y: u32,
    fill_premul: [u8; 4],
    tolerance: u8,
    clip: Option<&Selection>,
) -> Option<DirtyRect> {
    let w = layer.width as usize;
    let h = layer.height as usize;
    if x >= layer.width || y >= layer.height {
        return None;
    }
    if !clip_allows_point(clip, x as i32, y as i32) {
        return None;
    }
    let start = layer.pixel_premul(x, y);
    let tol = tolerance as i32;
    if (0..4).all(|i| (start[i] as i32 - fill_premul[i] as i32).abs() <= tol)
        && start == fill_premul
    {
        return None;
    }
    let mut visited = vec![0u8; w * h];
    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(256);
    stack.push((x as usize, y as usize));

    let mut min_x = w as i32;
    let mut min_y = h as i32;
    let mut max_x = -1i32;
    let mut max_y = -1i32;

    let can_fill = |pixels: &[u8], visited: &[u8], cx: usize, cy: usize| -> bool {
        let idx = cy * w + cx;
        if visited[idx] != 0 || !clip_allows_point(clip, cx as i32, cy as i32) {
            return false;
        }
        let p = idx * 4;
        (pixels[p] as i32 - start[0] as i32).abs() <= tol
            && (pixels[p + 1] as i32 - start[1] as i32).abs() <= tol
            && (pixels[p + 2] as i32 - start[2] as i32).abs() <= tol
            && (pixels[p + 3] as i32 - start[3] as i32).abs() <= tol
    };

    while let Some((sx, sy)) = stack.pop() {
        if !can_fill(&layer.pixels, &visited, sx, sy) {
            continue;
        }
        let mut lx = sx;
        while lx > 0 && can_fill(&layer.pixels, &visited, lx - 1, sy) {
            lx -= 1;
        }
        let mut rx = sx;
        while rx + 1 < w && can_fill(&layer.pixels, &visited, rx + 1, sy) {
            rx += 1;
        }
        let row = sy * w;
        visited[row + lx..=row + rx].fill(1);
        for cx in lx..=rx {
            let pi = (row + cx) * 4;
            layer.pixels[pi..pi + 4].copy_from_slice(&fill_premul);
        }
        min_x = min_x.min(lx as i32);
        max_x = max_x.max(rx as i32);
        min_y = min_y.min(sy as i32);
        max_y = max_y.max(sy as i32);

        for ny in [sy.checked_sub(1), (sy + 1 < h).then_some(sy + 1)]
            .into_iter()
            .flatten()
        {
            let mut in_span = false;
            for cx in lx..=rx {
                let ok = can_fill(&layer.pixels, &visited, cx, ny);
                if ok && !in_span {
                    stack.push((cx, ny));
                    in_span = true;
                } else if !ok {
                    in_span = false;
                }
            }
        }
    }

    if max_x < min_x {
        None
    } else {
        Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    }
}

pub fn copy_rect(layer: &Layer, x: i32, y: i32, rw: i32, rh: i32) -> Vec<u8> {
    let mut out = vec![0u8; (rw.max(0) * rh.max(0) * 4) as usize];
    if rw <= 0 || rh <= 0 {
        return out;
    }
    let lw = layer.width as i32;
    let lh = layer.height as i32;
    let col_start = (-x).max(0);
    let col_end = (lw - x).min(rw);
    let row_start = (-y).max(0);
    let row_end = (lh - y).min(rh);
    if col_end <= col_start || row_end <= row_start {
        return out;
    }
    let span_bytes = ((col_end - col_start) * 4) as usize;
    for row in row_start..row_end {
        let li = layer.idx((x + col_start) as u32, (y + row) as u32);
        let oi = ((row * rw + col_start) * 4) as usize;
        out[oi..oi + span_bytes].copy_from_slice(&layer.pixels[li..li + span_bytes]);
    }
    out
}

pub fn clear_rect(layer: &mut Layer, x: i32, y: i32, rw: i32, rh: i32) {
    let Some((x0, y0, cw, ch)) = clip_dirty_to_layer(layer, x, y, rw, rh) else {
        return;
    };
    let span_bytes = (cw * 4) as usize;
    for row in y0..y0 + ch {
        let li = layer.idx(x0 as u32, row as u32);
        layer.pixels[li..li + span_bytes].fill(0);
    }
}

pub fn paste_rect(layer: &mut Layer, x: i32, y: i32, rw: i32, rh: i32, data: &[u8]) {
    if rw <= 0 || rh <= 0 {
        return;
    }
    let lw = layer.width as i32;
    let lh = layer.height as i32;
    let col_start = (-x).max(0);
    let col_end = (lw - x).min(rw);
    let row_start = (-y).max(0);
    let row_end = (lh - y).min(rh);
    if col_end <= col_start || row_end <= row_start {
        return;
    }
    for row in row_start..row_end {
        let sy = (y + row) as u32;
        for col in col_start..col_end {
            let si = ((row * rw + col) * 4) as usize;
            if si + 4 > data.len() {
                continue;
            }
            let src = [data[si], data[si + 1], data[si + 2], data[si + 3]];
            if src[3] == 0 {
                continue;
            }
            let li = layer.idx((x + col) as u32, sy);
            blend_premul_pixel_int(&mut layer.pixels[li..li + 4], src);
        }
    }
}

/// Single-pass SDF rectangle (filled or outlined) — fast and uniform alpha with no corner/interior overlap buildup.
pub fn draw_rect_outline(
    layer: &mut Layer,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    filled: bool,
    eraser: bool,
    clip: Option<&Selection>,
    cache: &mut BrushStampCache,
) -> Option<DirtyRect> {
    if radius <= 0.0 || color[3] == 0 {
        return None;
    }
    cache.prepare(radius, hardness);
    let r = radius.max(0.55);
    let inv_r = 1.0 / r;
    let min_x = x0.min(x1);
    let max_x = x0.max(x1);
    let min_y = y0.min(y1);
    let max_y = y0.max(y1);

    let bx0 = (min_x - r - 1.0).floor() as i32;
    let by0 = (min_y - r - 1.0).floor() as i32;
    let bx1 = (max_x + r + 1.0).ceil() as i32;
    let by1 = (max_y + r + 1.0).ceil() as i32;

    let bounds = clip_loop_bounds(
        layer.width as i32,
        layer.height as i32,
        bx0,
        by0,
        bx1,
        by1,
        clip,
    )?;

    let layer_stride = layer.width as usize * 4;
    let falloff = &cache.falloff;

    for iy in bounds.y0..bounds.y1 {
        let py = iy as f64 + 0.5;
        let dy_out = (min_y - py).max(0.0).max(py - max_y);
        let dy_in = (py - min_y).min(max_y - py);
        let pix_row = iy as usize * layer_stride;
        for ix in bounds.x0..bounds.x1 {
            if let Some((clip_mask, clip_w)) = bounds.mask {
                if clip_mask[iy as usize * clip_w + ix as usize] == 0 {
                    continue;
                }
            }
            let px = ix as f64 + 0.5;
            let dx_out = (min_x - px).max(0.0).max(px - max_x);
            let aq = if filled {
                if dx_out <= 0.0 && dy_out <= 0.0 {
                    255u8
                } else {
                    let d = (dx_out * dx_out + dy_out * dy_out).sqrt();
                    if d >= r {
                        continue;
                    }
                    falloff.sample_u8((1.0 - d * inv_r).clamp(0.0, 1.0))
                }
            } else {
                let d = if dx_out > 0.0 || dy_out > 0.0 {
                    (dx_out * dx_out + dy_out * dy_out).sqrt()
                } else {
                    let dx_in = (px - min_x).min(max_x - px);
                    dx_in.min(dy_in).max(0.0)
                };
                if d >= r {
                    continue;
                }
                falloff.sample_u8((1.0 - d * inv_r).clamp(0.0, 1.0))
            };
            if aq == 0 {
                continue;
            }
            let i = pix_row + ix as usize * 4;
            apply_brush_pixel(&mut layer.pixels[i..i + 4], color, aq, eraser);
        }
    }

    Some((
        bounds.x0,
        bounds.y0,
        bounds.x1 - bounds.x0,
        bounds.y1 - bounds.y0,
    ))
}

/// Ramanujan's second approximation for ellipse circumference (a, b = semi-axes).
fn ellipse_perimeter_approx(a: f64, b: f64) -> f64 {
    let a = a.abs();
    let b = b.abs();
    if a < 1e-9 || b < 1e-9 {
        return 0.0;
    }
    let (maj, min) = if a >= b { (a, b) } else { (b, a) };
    let h = ((maj - min) / (maj + min)).powi(2);
    std::f64::consts::PI * (maj + min) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()))
}

/// Number of angular steps for previewing an outlined ellipse.
pub fn ellipse_outline_segment_count(rx: f64, ry: f64, brush_radius: f64, hardness: f64) -> i32 {
    let perim = ellipse_perimeter_approx(rx, ry);
    let step = brush_stroke_step(brush_radius, hardness);
    ((perim / step).ceil() as i32).clamp(8, 48_000)
}

/// Single-pass SDF ellipse (filled or outlined) — fast, uniform stroke width, and uniform alpha.
pub fn draw_ellipse(
    layer: &mut Layer,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    filled: bool,
    eraser: bool,
    clip: Option<&Selection>,
    cache: &mut BrushStampCache,
) -> Option<DirtyRect> {
    if radius <= 0.0 || color[3] == 0 {
        return None;
    }
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let rx = (x1 - x0).abs() * 0.5;
    let ry = (y1 - y0).abs() * 0.5;
    if rx < 0.5 || ry < 0.5 {
        return None;
    }
    cache.prepare(radius, hardness);
    let r = radius.max(0.55);
    let inv_r = 1.0 / r;
    let inv_rx = 1.0 / rx;
    let inv_ry = 1.0 / ry;
    let inv_rx2 = inv_rx * inv_rx;
    let inv_ry2 = inv_ry * inv_ry;
    let min_axis = rx.min(ry);

    let bx0 = (cx - rx - r - 1.0).floor() as i32;
    let bx1 = (cx + rx + r + 1.0).ceil() as i32;
    let by0 = (cy - ry - r - 1.0).floor() as i32;
    let by1 = (cy + ry + r + 1.0).ceil() as i32;

    let bounds = clip_loop_bounds(
        layer.width as i32,
        layer.height as i32,
        bx0,
        by0,
        bx1,
        by1,
        clip,
    )?;

    let layer_stride = layer.width as usize * 4;
    let falloff = &cache.falloff;

    for iy in bounds.y0..bounds.y1 {
        let ey = (iy as f64 + 0.5 - cy).abs();
        let ny = ey * inv_ry;
        let ny2 = ny * ny;
        let gy = ey * inv_ry2;
        let gy2 = gy * gy;
        let pix_row = iy as usize * layer_stride;
        for ix in bounds.x0..bounds.x1 {
            if let Some((clip_mask, clip_w)) = bounds.mask {
                if clip_mask[iy as usize * clip_w + ix as usize] == 0 {
                    continue;
                }
            }
            let ex = (ix as f64 + 0.5 - cx).abs();
            let nx = ex * inv_rx;
            let k0 = (nx * nx + ny2).sqrt();
            let gx = ex * inv_rx2;
            let k1 = (gx * gx + gy2).sqrt();
            let signed_d = if k1 > 1e-9 {
                k0 * (k0 - 1.0) / k1
            } else {
                -min_axis
            };
            let aq = if filled {
                if signed_d <= 0.0 {
                    255u8
                } else if signed_d < r {
                    falloff.sample_u8((1.0 - signed_d * inv_r).clamp(0.0, 1.0))
                } else {
                    continue;
                }
            } else {
                let d = signed_d.abs();
                if d >= r {
                    continue;
                }
                falloff.sample_u8((1.0 - d * inv_r).clamp(0.0, 1.0))
            };
            if aq == 0 {
                continue;
            }
            let i = pix_row + ix as usize * 4;
            apply_brush_pixel(&mut layer.pixels[i..i + 4], color, aq, eraser);
        }
    }

    Some((
        bounds.x0,
        bounds.y0,
        bounds.x1 - bounds.x0,
        bounds.y1 - bounds.y0,
    ))
}

/// Sample composite buffer (premul) at integer pixel → straight RGBA for UI.
/// Returns `None` if `(x, y)` is outside the canvas.
pub fn sample_composite_premul(
    comp: &[u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Option<[u8; 4]> {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return None;
    }
    let idx = ((y as u32 * width + x as u32) * 4) as usize;
    let pr = comp[idx];
    let pg = comp[idx + 1];
    let pb = comp[idx + 2];
    let pa = comp[idx + 3];
    if pa == 0 {
        return Some([0, 0, 0, 0]);
    }
    if pa == 255 {
        return Some([pr, pg, pb, 255]);
    }
    Some([
        ((pr as u32 * 255 + pa as u32 / 2) / pa as u32).min(255) as u8,
        ((pg as u32 * 255 + pa as u32 / 2) / pa as u32).min(255) as u8,
        ((pb as u32 * 255 + pa as u32 / 2) / pa as u32).min(255) as u8,
        pa,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_square_does_not_double_apply_alpha() {
        let mut layer = Layer::new(4, 4, "test");
        stamp_square(&mut layer, 1.5, 1.5, 1.0, [200, 100, 50, 128], false, None);
        let px = layer.pixel_premul(1, 1);
        assert_eq!(px[3], 128);
        assert!((px[0] as i32 - 100).abs() <= 1);
        assert!((px[1] as i32 - 50).abs() <= 1);
        assert!((px[2] as i32 - 25).abs() <= 1);
    }

    #[test]
    fn stamp_circle_center_does_not_double_apply_alpha() {
        let mut layer = Layer::new(16, 16, "test");
        let mut cache = BrushStampCache::new();
        stamp_circle_cached(&mut layer, 8.5, 8.5, 4.0, 1.0, [200, 100, 50, 128], false, None, &mut cache);
        let px = layer.pixel_premul(8, 8);
        assert_eq!(px[3], 128);
        assert!((px[0] as i32 - 100).abs() <= 1);
    }

    #[test]
    fn filled_rect_has_uniform_semitransparent_alpha() {
        let mut layer = Layer::new(40, 40, "test");
        let mut cache = BrushStampCache::new();
        draw_rect_outline(
            &mut layer,
            5.0,
            5.0,
            30.0,
            30.0,
            3.0,
            0.5,
            [255, 0, 0, 128],
            true,
            false,
            None,
            &mut cache,
        );
        // Every interior pixel should have exact alpha 128 (no overlapping buildup).
        for y in 6..29 {
            for x in 6..29 {
                let px = layer.pixel_premul(x, y);
                assert_eq!(px[3], 128, "at ({x},{y}): {:?}", px);
                assert_eq!(px[0], 128, "at ({x},{y}): {:?}", px);
            }
        }
    }

    #[test]
    fn scanline_flood_fill_respects_clip_and_tolerance() {
        let mut layer = Layer::new(10, 10, "test");
        layer.pixels.fill(255);
        // Draw a vertical barrier at x=5.
        for y in 0..10 {
            let i = layer.idx(5, y);
            layer.pixels[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
        let clip = Selection::Rect(1, 1, 8, 8);
        let dirty = flood_fill(&mut layer, 2, 2, [255, 0, 0, 255], 0, Some(&clip));
        assert_eq!(dirty, Some((1, 1, 4, 8)));
        assert_eq!(layer.pixel_premul(0, 0), [255, 255, 255, 255]);
        assert_eq!(layer.pixel_premul(2, 2), [255, 0, 0, 255]);
        assert_eq!(layer.pixel_premul(6, 2), [255, 255, 255, 255]);
    }

    #[test]
    fn hardness_direction_is_not_inverted() {
        let mut soft = Layer::new(32, 32, "soft");
        let mut hard = Layer::new(32, 32, "hard");
        let mut cache = BrushStampCache::new();
        stamp_circle_cached(&mut soft, 16.0, 16.0, 10.0, 0.0, [255, 0, 0, 255], false, None, &mut cache);
        stamp_circle_cached(&mut hard, 16.0, 16.0, 10.0, 1.0, [255, 0, 0, 255], false, None, &mut cache);
        // At 6px from center (inside 10px radius): hard brush should be solid (255), soft brush faded.
        assert_eq!(hard.pixel_premul(22, 16)[3], 255);
        assert!(soft.pixel_premul(22, 16)[3] < 100);
    }

    #[test]
    fn line_and_ellipse_have_uniform_semitransparent_alpha() {
        let mut layer = Layer::new(64, 64, "test");
        let mut cache = BrushStampCache::new();
        draw_line_capsule(&mut layer, 8.0, 32.0, 56.0, 32.0, 6.0, 1.0, [0, 200, 0, 128], false, None, &mut cache);
        assert_eq!(layer.pixel_premul(32, 32)[3], 128);

        let mut layer2 = Layer::new(64, 64, "test2");
        draw_ellipse(&mut layer2, 10.0, 10.0, 54.0, 54.0, 4.0, 1.0, [0, 0, 255, 128], true, false, None, &mut cache);
        assert_eq!(layer2.pixel_premul(32, 32)[3], 128);
    }

    #[test]
    fn scanline_flood_select_returns_exact_bbox() {
        let mut layer = Layer::new(10, 10, "test");
        for y in 2..6 {
            for x in 3..8 {
                let i = layer.idx(x, y);
                layer.pixels[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);
            }
        }
        let (mask, bbox) = flood_select_mask(&layer, 4, 3, 0);
        assert_eq!(bbox, Some((3, 2, 5, 4)));
        assert_eq!(mask.iter().filter(|&&v| v != 0).count(), 20);
    }
}
