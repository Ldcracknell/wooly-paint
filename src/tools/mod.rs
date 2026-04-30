use crate::document::Layer;
use crate::selection::Selection;

#[inline]
fn clip_allows(clip: Option<&Selection>, ix: i32, iy: i32) -> bool {
    match clip {
        None => true,
        Some(s) => s.contains_point(ix as f64 + 0.5, iy as f64 + 0.5),
    }
}

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

fn straight_to_premul_rgba(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    let a32 = a as u32;
    [
        (r as u32 * a32 / 255) as u8,
        (g as u32 * a32 / 255) as u8,
        (b as u32 * a32 / 255) as u8,
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

#[inline]
fn blend_premul_pixel_int(dst: &mut [u8], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 0 {
        return;
    }
    let inv = 255 - sa;
    dst[0] = (src[0] as u32 + (dst[0] as u32 * inv + 127) / 255).min(255) as u8;
    dst[1] = (src[1] as u32 + (dst[1] as u32 * inv + 127) / 255).min(255) as u8;
    dst[2] = (src[2] as u32 + (dst[2] as u32 * inv + 127) / 255).min(255) as u8;
    dst[3] = (sa + (dst[3] as u32 * inv + 127) / 255).min(255) as u8;
}

fn blend_premul_pixel(dst: &mut [u8; 4], src: [u8; 4]) {
    blend_premul_pixel_int(dst, src);
}

/// Falloff LUT for a fixed hardness; building it is expensive, so reuse across many stamps
/// (filled shapes, polyline dabs) instead of recomputing inside each [`stamp_circle`].
struct BrushFalloffCache {
    lut: [f64; 256],
    lift_scale: f64,
}

impl BrushFalloffCache {
    fn new(hardness: f64) -> Self {
        let hard = hardness.clamp(0.0, 1.0);
        let gamma = 0.42 + 19.5 * hard * hard;
        let mut lut = [0f64; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let u = i as f64 / 255.0;
            *slot = u.powf(gamma);
        }
        let lift_scale = (1.0 - hard) * 0.22;
        Self { lut, lift_scale }
    }
}

const BRUSH_SUBPIXEL_STEPS: usize = 4;
const BRUSH_SUBPIXEL_MASKS: usize = BRUSH_SUBPIXEL_STEPS * BRUSH_SUBPIXEL_STEPS;

struct BrushStampMask {
    rel_x0: i32,
    rel_y0: i32,
    width: i32,
    height: i32,
    alpha: Vec<u8>,
}

impl BrushStampMask {
    fn new(
        radius: f64,
        color_alpha: u8,
        falloff: &BrushFalloffCache,
        qx: usize,
        qy: usize,
    ) -> Self {
        let r = radius.max(0.5);
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
            for col in 0..width {
                let ix = x0 + col;
                let dx = ix as f64 + 0.5 - cx;
                let dy = iy as f64 + 0.5 - cy;
                let d2 = dx * dx + dy * dy;
                if d2 > r2 {
                    continue;
                }
                let u = (1.0 - d2.sqrt() * inv_r).clamp(0.0, 1.0);
                if u <= 0.0 {
                    continue;
                }
                let ui = (u * 255.0 + 0.5).clamp(0.0, 255.0) as usize;
                let body = falloff.lut[ui];
                let lift = falloff.lift_scale * u;
                let a_straight = (body + lift).min(1.0);
                let aq = (a_straight * color_alpha as f64).round().clamp(0.0, 255.0) as u8;
                alpha[(row * width + col) as usize] = aq;
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

struct BrushStampCache {
    radius: f64,
    color_alpha: u8,
    falloff: BrushFalloffCache,
    masks: Vec<Option<BrushStampMask>>,
}

impl BrushStampCache {
    fn new(radius: f64, hardness: f64, color_alpha: u8) -> Self {
        Self {
            radius,
            color_alpha,
            falloff: BrushFalloffCache::new(hardness),
            masks: (0..BRUSH_SUBPIXEL_MASKS).map(|_| None).collect(),
        }
    }

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
            self.masks[idx] = Some(BrushStampMask::new(
                self.radius,
                self.color_alpha,
                &self.falloff,
                qx,
                qy,
            ));
        }
        self.masks[idx].as_ref().expect("brush mask inserted")
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
    if radius <= 0.0 {
        return None;
    }
    let w = layer.width as i32;
    let h = layer.height as i32;
    let base = straight_to_premul_rgba(color[0], color[1], color[2], color[3]);
    let mask = cache.mask(cx, cy);
    let x0 = cx.floor() as i32 + mask.rel_x0;
    let y0 = cy.floor() as i32 + mask.rel_y0;
    let x1 = x0 + mask.width;
    let y1 = y0 + mask.height;
    let dirty = clip_dirty_to_layer(layer, x0, y0, mask.width, mask.height);

    for iy in y0.max(0)..y1.min(h) {
        let mask_row = (iy - y0) * mask.width;
        for ix in x0.max(0)..x1.min(w) {
            let aq = mask.alpha[(mask_row + ix - x0) as usize];
            if aq == 0 {
                continue;
            }
            if !clip_allows(clip, ix, iy) {
                continue;
            }
            let i = layer.idx(ix as u32, iy as u32);
            if eraser {
                let inv = 255u32 - aq as u32;
                for c in 0..4 {
                    layer.pixels[i + c] =
                        ((layer.pixels[i + c] as u32 * inv + 127) / 255).min(255) as u8;
                }
            } else {
                let src = [
                    ((base[0] as u32 * aq as u32 + 127) / 255).min(255) as u8,
                    ((base[1] as u32 * aq as u32 + 127) / 255).min(255) as u8,
                    ((base[2] as u32 * aq as u32 + 127) / 255).min(255) as u8,
                    ((base[3] as u32 * aq as u32 + 127) / 255).min(255) as u8,
                ];
                blend_premul_pixel_int(&mut layer.pixels[i..i + 4], src);
            }
        }
    }
    dirty
}

/// `color` straight RGBA; composites with existing premultiplied canvas.
pub fn stamp_circle(
    layer: &mut Layer,
    cx: f64,
    cy: f64,
    radius: f64,
    hardness: f64,
    color: [u8; 4],
    eraser: bool,
    clip: Option<&Selection>,
) -> Option<DirtyRect> {
    if radius <= 0.0 {
        return None;
    }
    let mut cache = BrushStampCache::new(radius, hardness, color[3]);
    stamp_circle_with_falloff(layer, cx, cy, radius, color, eraser, &mut cache, clip)
}

fn brush_stroke_step(radius: f64, hardness: f64) -> f64 {
    let h = hardness.clamp(0.0, 1.0);
    let base = (radius * 0.35).max(0.5);
    let tight = 0.04 + 0.96 * h * h;
    let mut step = base * tight;
    if h < 0.25 {
        // Very soft brushes reveal individual dabs unless spacing is dense, but a fixed
        // sub-pixel cap makes huge brushes do massive redundant work.
        let large_brush_relief = (radius.max(0.5) - 12.0).max(0.0) * 0.08;
        let soft_cap = 0.18 + large_brush_relief + 6.0 * h * h;
        step = step.min(soft_cap);
    }
    step.max(0.05)
}

/// Minimum spacing between dab centers: softer brushes need denser dabs, but an absolute floor
/// avoids pathological segment counts when the pointer jumps (fast moves, low event rate).
fn brush_min_step(radius: f64, hardness: f64) -> f64 {
    let h = hardness.clamp(0.0, 1.0);
    let r = radius.max(0.5);
    (r * (0.07 + 0.93 * h * h)).max(0.12)
}

fn brush_spacing(radius: f64, hardness: f64) -> f64 {
    let r = radius.max(0.5);
    let h = hardness.clamp(0.0, 1.0);
    let large_soft_floor = if h < 0.25 {
        let ramp = ((r - 12.0) / 20.0).clamp(0.0, 1.0);
        brush_min_step(radius, hardness) * ramp
    } else {
        0.0
    };
    brush_stroke_step(radius, hardness)
        .max(large_soft_floor)
        .min(r * 0.33)
        .max(0.05)
}

pub fn stroke_line(
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
) -> Option<DirtyRect> {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len <= 1e-9 {
        return stamp_circle(layer, x0, y0, radius, hardness, color, eraser, clip);
    }
    let h = hardness.clamp(0.0, 1.0);
    let step = brush_spacing(radius, hardness);
    let n_f = (len / step).ceil();
    if !n_f.is_finite() || n_f <= 0.0 {
        return stamp_circle(layer, x0, y0, radius, hardness, color, eraser, clip);
    }
    // Cap dab count per segment so a single long chord cannot freeze the UI.
    let max_n = (50_000.0 / (0.15 + 0.85 * h * h)) as u64;
    let n = (n_f as u64).min(max_n).min(200_000) as usize;
    if n == 0 {
        return stamp_circle(layer, x0, y0, radius, hardness, color, eraser, clip);
    }
    let mut cache = BrushStampCache::new(radius, hardness, color[3]);
    let mut dirty = None;
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        dirty = union_dirty(
            dirty,
            stamp_circle_with_falloff(layer, x, y, radius, color, eraser, &mut cache, clip),
        );
    }
    dirty
}

pub fn stroke_line_spaced(
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

    let mut cache = BrushStampCache::new(radius, hardness, color[3]);
    let mut dirty = None;
    let mut d = *next_dab_distance;
    while d <= len {
        let t = d / len;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        dirty = union_dirty(
            dirty,
            stamp_circle_with_falloff(layer, x, y, radius, color, eraser, &mut cache, clip),
        );
        d += spacing;
    }
    *next_dab_distance = d - len;
    dirty
}

pub fn stroke_quadratic_spaced(
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
    let mut cache = BrushStampCache::new(radius, hardness, color[3]);
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
                        &mut cache,
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
    if size <= 0.0 {
        return None;
    }
    let mut s = size.floor() as i32;
    if s < 1 {
        s = 1;
    }
    let w = layer.width as i32;
    let h = layer.height as i32;
    let ax = cx.floor() as i32;
    let ay = cy.floor() as i32;
    let x0 = ax - (s - 1) / 2;
    let y0 = ay - (s - 1) / 2;
    let x1 = x0 + s;
    let y1 = y0 + s;
    let base = straight_to_premul_rgba(color[0], color[1], color[2], color[3]);
    let alpha = color[3] as f64 / 255.0;
    if alpha <= 0.0 {
        return None;
    }
    let dirty = clip_dirty_to_layer(layer, x0, y0, s, s);

    for iy in y0.max(0)..y1.min(h) {
        for ix in x0.max(0)..x1.min(w) {
            if !clip_allows(clip, ix, iy) {
                continue;
            }
            let i = layer.idx(ix as u32, iy as u32);
            if eraser {
                let inv = (1.0 - alpha) as f32;
                for c in 0..4 {
                    layer.pixels[i + c] =
                        (layer.pixels[i + c] as f32 * inv).round().clamp(0.0, 255.0) as u8;
                }
            } else {
                let src = [
                    (base[0] as f64 * alpha).round() as u8,
                    (base[1] as f64 * alpha).round() as u8,
                    (base[2] as f64 * alpha).round() as u8,
                    (base[3] as f64 * alpha).round() as u8,
                ];
                let mut p = [
                    layer.pixels[i],
                    layer.pixels[i + 1],
                    layer.pixels[i + 2],
                    layer.pixels[i + 3],
                ];
                blend_premul_pixel(&mut p, src);
                layer.pixels[i..i + 4].copy_from_slice(&p);
            }
        }
    }
    dirty
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
    bresenham_visit(p0x, p0y, p1x, p1y, |ix, iy| {
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
/// Second value is tight bounds of selected cells when any, same coordinate space as [`region_tight_bbox`].
pub fn flood_select_mask(
    layer: &Layer,
    x: u32,
    y: u32,
    tolerance: u8,
) -> (Vec<u8>, Option<(i32, i32, i32, i32)>) {
    let w = layer.width;
    let h = layer.height;
    let len = (w * h) as usize;
    let mut mask = vec![0u8; len];
    if x >= w || y >= h {
        return (mask, None);
    }
    let wi = w as i32;
    let hi = h as i32;
    let mut min_x = wi;
    let mut min_y = hi;
    let mut max_x = -1i32;
    let mut max_y = -1i32;
    let start = layer.pixel_premul(x, y);
    let tol = tolerance as i32;
    let match_start = |p: [u8; 4]| (0..4).all(|i| (p[i] as i32 - start[i] as i32).abs() <= tol);
    let mut stack: Vec<(u32, u32)> = vec![(x, y)];
    while let Some((cx, cy)) = stack.pop() {
        let idx = (cy * w + cx) as usize;
        if mask[idx] != 0 {
            continue;
        }
        let p = layer.pixel_premul(cx, cy);
        if !match_start(p) {
            continue;
        }
        mask[idx] = 1;
        let xi = cx as i32;
        let yi = cy as i32;
        min_x = min_x.min(xi);
        min_y = min_y.min(yi);
        max_x = max_x.max(xi);
        max_y = max_y.max(yi);
        if cx > 0 {
            stack.push((cx - 1, cy));
        }
        if cx + 1 < w {
            stack.push((cx + 1, cy));
        }
        if cy > 0 {
            stack.push((cx, cy - 1));
        }
        if cy + 1 < h {
            stack.push((cx, cy + 1));
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
        for x in 0..width {
            if mask[(y * width + x) as usize] != 0 {
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

/// Clear premultiplied pixels where `mask` is non-zero (`mask` is document-sized).
pub fn clear_region_masked(layer: &mut Layer, mask: &[u8]) {
    let w = layer.width as usize;
    let h = layer.height as usize;
    let expected = w * h;
    if mask.len() != expected {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            let mi = y * w + x;
            if mask[mi] != 0 {
                let i = mi * 4;
                layer.pixels[i..i + 4].fill(0);
            }
        }
    }
}

pub fn flood_fill(
    layer: &mut Layer,
    x: u32,
    y: u32,
    fill_premul: [u8; 4],
    tolerance: u8,
    clip: Option<&Selection>,
) {
    let w = layer.width;
    let h = layer.height;
    if x >= w || y >= h {
        return;
    }
    if !clip_allows(clip, x as i32, y as i32) {
        return;
    }
    let start = layer.pixel_premul(x, y);
    let tol = tolerance as i32;
    let match_start = |p: [u8; 4]| (0..4).all(|i| (p[i] as i32 - start[i] as i32).abs() <= tol);
    if (0..4).all(|i| (start[i] as i32 - fill_premul[i] as i32).abs() <= tol) {
        return;
    }
    let mut visited = vec![false; (w * h) as usize];
    let mut stack = vec![(x, y)];
    while let Some((cx, cy)) = stack.pop() {
        let idx = (cy * w + cx) as usize;
        if visited[idx] {
            continue;
        }
        let p = layer.pixel_premul(cx, cy);
        if !match_start(p) {
            continue;
        }
        visited[idx] = true;
        let i = layer.idx(cx, cy);
        layer.pixels[i..i + 4].copy_from_slice(&fill_premul);
        if cx > 0 && clip_allows(clip, (cx - 1) as i32, cy as i32) {
            stack.push((cx - 1, cy));
        }
        if cx + 1 < w && clip_allows(clip, (cx + 1) as i32, cy as i32) {
            stack.push((cx + 1, cy));
        }
        if cy > 0 && clip_allows(clip, cx as i32, (cy - 1) as i32) {
            stack.push((cx, cy - 1));
        }
        if cy + 1 < h && clip_allows(clip, cx as i32, (cy + 1) as i32) {
            stack.push((cx, cy + 1));
        }
    }
}

pub fn copy_rect(layer: &Layer, x: i32, y: i32, rw: i32, rh: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity((rw * rh * 4) as usize);
    for row in 0..rh {
        for col in 0..rw {
            let sx = x + col;
            let sy = y + row;
            if sx >= 0 && sy >= 0 && sx < layer.width as i32 && sy < layer.height as i32 {
                let p = layer.pixel_premul(sx as u32, sy as u32);
                out.extend_from_slice(&p);
            } else {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    out
}

pub fn clear_rect(layer: &mut Layer, x: i32, y: i32, rw: i32, rh: i32) {
    for row in 0..rh {
        for col in 0..rw {
            let sx = x + col;
            let sy = y + row;
            if sx >= 0 && sy >= 0 && sx < layer.width as i32 && sy < layer.height as i32 {
                let i = layer.idx(sx as u32, sy as u32);
                layer.pixels[i..i + 4].fill(0);
            }
        }
    }
}

pub fn paste_rect(layer: &mut Layer, x: i32, y: i32, rw: i32, rh: i32, data: &[u8]) {
    let mut i = 0usize;
    for row in 0..rh {
        for col in 0..rw {
            let sx = x + col;
            let sy = y + row;
            if sx >= 0 && sy >= 0 && sx < layer.width as i32 && sy < layer.height as i32 {
                if i + 4 <= data.len() {
                    let src = [data[i], data[i + 1], data[i + 2], data[i + 3]];
                    let li = layer.idx(sx as u32, sy as u32);
                    let mut dst = [
                        layer.pixels[li],
                        layer.pixels[li + 1],
                        layer.pixels[li + 2],
                        layer.pixels[li + 3],
                    ];
                    blend_premul_pixel(&mut dst, src);
                    layer.pixels[li..li + 4].copy_from_slice(&dst);
                }
            }
            i += 4;
        }
    }
}

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
) {
    let min_x = x0.min(x1);
    let max_x = x0.max(x1);
    let min_y = y0.min(y1);
    let max_y = y0.max(y1);
    if filled {
        let step = brush_stroke_step(radius, hardness)
            .max(brush_min_step(radius, hardness))
            .max(radius * 0.22)
            .max(0.75);
        let mut cache = BrushStampCache::new(radius, hardness, color[3]);
        let mut y = min_y.floor();
        let y1 = max_y.ceil();
        let x0f = min_x;
        let x1f = max_x;
        let y0f = min_y;
        let y1f = max_y;
        while y <= y1 {
            let mut x = min_x.floor();
            let x1 = max_x.ceil();
            while x <= x1 {
                let px = x + 0.5;
                let py = y + 0.5;
                if px >= x0f && px <= x1f && py >= y0f && py <= y1f {
                    let _ = stamp_circle_with_falloff(
                        layer, px, py, radius, color, eraser, &mut cache, clip,
                    );
                }
                x += step;
            }
            y += step;
        }
    } else {
        stroke_line(
            layer, min_x, min_y, max_x, min_y, radius, hardness, color, eraser, clip,
        );
        stroke_line(
            layer, max_x, min_y, max_x, max_y, radius, hardness, color, eraser, clip,
        );
        stroke_line(
            layer, max_x, max_y, min_x, max_y, radius, hardness, color, eraser, clip,
        );
        stroke_line(
            layer, min_x, max_y, min_x, min_y, radius, hardness, color, eraser, clip,
        );
    }
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

/// Number of angular steps for an outlined ellipse (`draw_ellipse` outline path).
/// Matches perimeter-based spacing vs. brush radius so previews use the same polygon density.
pub fn ellipse_outline_segment_count(rx: f64, ry: f64, brush_radius: f64, hardness: f64) -> i32 {
    let perim = ellipse_perimeter_approx(rx, ry);
    let step = brush_stroke_step(brush_radius, hardness);
    ((perim / step).ceil() as i32).clamp(8, 48_000)
}

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
) {
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let rx = (x1 - x0).abs() * 0.5;
    let ry = (y1 - y0).abs() * 0.5;
    if rx < 0.5 || ry < 0.5 {
        return;
    }
    let x_min = (cx - rx - radius).floor() as i32;
    let x_max = (cx + rx + radius).ceil() as i32;
    let y_min = (cy - ry - radius).floor() as i32;
    let y_max = (cy + ry + radius).ceil() as i32;
    let w = layer.width as i32;
    let h = layer.height as i32;
    if filled {
        let step = brush_stroke_step(radius, hardness)
            .max(brush_min_step(radius, hardness))
            .max(radius * 0.18)
            .max(0.75);
        let mut cache = BrushStampCache::new(radius, hardness, color[3]);
        let y_end = y_max.min(h) as f64;
        let x_end = x_max.min(w) as f64;
        let mut y = y_min.max(0) as f64;
        while y < y_end {
            let mut x = x_min.max(0) as f64;
            while x < x_end {
                let px = x + 0.5;
                let py = y + 0.5;
                let nx = (px - cx) / rx;
                let ny = (py - cy) / ry;
                if nx * nx + ny * ny <= 1.0 {
                    let _ = stamp_circle_with_falloff(
                        layer, px, py, radius, color, eraser, &mut cache, clip,
                    );
                }
                x += step;
            }
            y += step;
        }
    } else {
        let n = ellipse_outline_segment_count(rx, ry, radius, hardness);
        let mut cache = BrushStampCache::new(radius, hardness, color[3]);
        for i in 0..=n {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            let px = cx + rx * t.cos();
            let py = cy + ry * t.sin();
            let _ =
                stamp_circle_with_falloff(layer, px, py, radius, color, eraser, &mut cache, clip);
        }
    }
}

/// Sample composite buffer (premul) at integer pixel → straight RGBA for UI.
pub fn sample_composite_premul(comp: &[u8], width: u32, height: u32, x: i32, y: i32) -> [u8; 4] {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return [0, 0, 0, 255];
    }
    let idx = ((y as u32 * width + x as u32) * 4) as usize;
    let pr = comp[idx];
    let pg = comp[idx + 1];
    let pb = comp[idx + 2];
    let pa = comp[idx + 3];
    if pa == 0 {
        return [0, 0, 0, 0];
    }
    [
        ((pr as u32 * 255 + pa as u32 / 2) / pa as u32).min(255) as u8,
        ((pg as u32 * 255 + pa as u32 / 2) / pa as u32).min(255) as u8,
        ((pb as u32 * 255 + pa as u32 / 2) / pa as u32).min(255) as u8,
        pa,
    ]
}
