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
    Move,
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
    let inner = r * hardness.clamp(0.0, 1.0);
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
            let alpha: f64 = ((r - d) / (r - inner).max(1e-6)).clamp(0.0, 1.0)
                * color[3] as f64 / 255.0;
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
    let step = (radius * 0.35).max(0.5);
    let n = (len / step).ceil() as i32;
    if n <= 0 {
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
    let w = layer.width as i32;
    let h = layer.height as i32;
    let half = size * 0.5;
    let base = straight_to_premul_rgba(color[0], color[1], color[2], color[3]);
    let alpha = color[3] as f64 / 255.0;
    if alpha <= 0.0 {
        return;
    }
    let x0 = (cx - half).floor() as i32;
    let y0 = (cy - half).floor() as i32;
    let x1 = (cx + half).ceil() as i32;
    let y1 = (cy + half).ceil() as i32;

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
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    let step = (size * 0.35).max(0.5);
    let n = (len / step).ceil() as i32;
    if n <= 0 {
        stamp_square(layer, x0, y0, size, color, eraser);
        return;
    }
    for i in 0..=n {
        let t = i as f64 / n as f64;
        stamp_square(layer, x0 + dx * t, y0 + dy * t, size, color, eraser);
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
        let steps = ((rx + ry) * 0.5).max(8.0).min(360.0) as i32;
        for i in 0..=steps {
            let t = std::f64::consts::TAU * i as f64 / steps as f64;
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
