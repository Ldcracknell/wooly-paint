use crate::document::Layer;

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

fn blend_premul_pixel(dst: &mut [u8; 4], src: [u8; 4]) {
    let sa = src[3] as f32 / 255.0;
    if sa <= 0.0 {
        return;
    }
    let inv = 1.0 - sa;
    for i in 0..3 {
        dst[i] = (src[i] as f32 + dst[i] as f32 * inv)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    dst[3] = ((sa + dst[3] as f32 / 255.0 * inv) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8;
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
) {
    if radius <= 0.0 {
        return;
    }
    let w = layer.width as i32;
    let h = layer.height as i32;
    let r = radius.max(0.5);
    let hard = hardness.clamp(0.0, 1.0);
    let x0 = (cx - r - 1.0).floor() as i32;
    let y0 = (cy - r - 1.0).floor() as i32;
    let x1 = (cx + r + 1.0).ceil() as i32;
    let y1 = (cy + r + 1.0).ceil() as i32;
    let base = straight_to_premul_rgba(color[0], color[1], color[2], color[3]);

    for iy in y0.max(0)..y1.min(h) {
        for ix in x0.max(0)..x1.min(w) {
            let dx = ix as f64 + 0.5 - cx;
            let dy = iy as f64 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d > r {
                continue;
            }
            let u = (1.0 - d / r).clamp(0.0, 1.0);
            if u <= 0.0 {
                continue;
            }
            // Gamma ramps gently with hardness so mid-slider stays usable (no exponential 125^h).
            let gamma = 0.42 + 19.5 * hard * hard;
            let body = u.powf(gamma);
            let lift = (1.0 - hard) * 0.22 * u;
            let a_straight = (body + lift).min(1.0);
            let alpha: f64 = a_straight * color[3] as f64 / 255.0;
            if alpha <= 0.0 {
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
}

fn brush_stroke_step(radius: f64, hardness: f64) -> f64 {
    let h = hardness.clamp(0.0, 1.0);
    let base = (radius * 0.35).max(0.5);
    let tight = 0.04 + 0.96 * h * h;
    (base * tight).max(0.05)
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
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len <= 1e-9 {
        stamp_circle(layer, x0, y0, radius, hardness, color, eraser);
        return;
    }
    let r = radius.max(0.5);
    let mut step = brush_stroke_step(radius, hardness);
    step = step.min(r * 0.33).max(0.06);
    let n_f = (len / step).ceil();
    if !n_f.is_finite() || n_f <= 0.0 {
        stamp_circle(layer, x0, y0, radius, hardness, color, eraser);
        return;
    }
    let n = (n_f as u64).min(2_000_000) as usize;
    if n == 0 {
        stamp_circle(layer, x0, y0, radius, hardness, color, eraser);
        return;
    }
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        stamp_circle(layer, x, y, radius, hardness, color, eraser);
    }
}

pub fn stamp_square(
    layer: &mut Layer,
    cx: f64,
    cy: f64,
    size: f64,
    color: [u8; 4],
    eraser: bool,
) {
    if size <= 0.0 {
        return;
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
        return;
    }

    for iy in y0.max(0)..y1.min(h) {
        for ix in x0.max(0)..x1.min(w) {
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
) {
    let p0x = x0.floor() as i32;
    let p0y = y0.floor() as i32;
    let p1x = x1.floor() as i32;
    let p1y = y1.floor() as i32;
    bresenham_visit(p0x, p0y, p1x, p1y, |ix, iy| {
        stamp_square(
            layer,
            ix as f64 + 0.5,
            iy as f64 + 0.5,
            size,
            color,
            eraser,
        );
    });
}

/// Connected pixels matching `layer[(x,y)]` within `tolerance` (premul RGBA). Does not modify the layer.
pub fn flood_select_mask(layer: &Layer, x: u32, y: u32, tolerance: u8) -> Vec<u8> {
    let w = layer.width;
    let h = layer.height;
    let len = (w * h) as usize;
    let mut mask = vec![0u8; len];
    if x >= w || y >= h {
        return mask;
    }
    let start = layer.pixel_premul(x, y);
    let tol = tolerance as i32;
    let match_start =
        |p: [u8; 4]| (0..4).all(|i| (p[i] as i32 - start[i] as i32).abs() <= tol);
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
    mask
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

pub fn flood_fill(layer: &mut Layer, x: u32, y: u32, fill_premul: [u8; 4], tolerance: u8) {
    let w = layer.width;
    let h = layer.height;
    if x >= w || y >= h {
        return;
    }
    let start = layer.pixel_premul(x, y);
    let tol = tolerance as i32;
    let match_start = |p: [u8; 4]| {
        (0..4).all(|i| (p[i] as i32 - start[i] as i32).abs() <= tol)
    };
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
                    let src = [
                        data[i],
                        data[i + 1],
                        data[i + 2],
                        data[i + 3],
                    ];
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
) {
    let min_x = x0.min(x1);
    let max_x = x0.max(x1);
    let min_y = y0.min(y1);
    let max_y = y0.max(y1);
    if filled {
        for iy in min_y.floor() as i32..=max_y.ceil() as i32 {
            for ix in min_x.floor() as i32..=max_x.ceil() as i32 {
                let px = ix as f64 + 0.5;
                let py = iy as f64 + 0.5;
                if px >= min_x && px <= max_x && py >= min_y && py <= max_y {
                    stamp_circle(layer, px, py, radius, hardness, color, eraser);
                }
            }
        }
    } else {
        stroke_line(layer, min_x, min_y, max_x, min_y, radius, hardness, color, eraser);
        stroke_line(layer, max_x, min_y, max_x, max_y, radius, hardness, color, eraser);
        stroke_line(layer, max_x, max_y, min_x, max_y, radius, hardness, color, eraser);
        stroke_line(layer, min_x, max_y, min_x, min_y, radius, hardness, color, eraser);
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
        for iy in y_min.max(0)..y_max.min(h) {
            for ix in x_min.max(0)..x_max.min(w) {
                let px = ix as f64 + 0.5;
                let py = iy as f64 + 0.5;
                let nx = (px - cx) / rx;
                let ny = (py - cy) / ry;
                if nx * nx + ny * ny <= 1.0 {
                    stamp_circle(layer, px, py, radius, hardness, color, eraser);
                }
            }
        }
    } else {
        let n = ellipse_outline_segment_count(rx, ry, radius, hardness);
        for i in 0..=n {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            let px = cx + rx * t.cos();
            let py = cy + ry * t.sin();
            stamp_circle(layer, px, py, radius, hardness, color, eraser);
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
