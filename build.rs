//! Rasterize Lucide SVGs (ISC, see assets/cursors/svg).
//! Hotspots are SVG user-space points (24×24 viewBox) mapped with the same transform as the glyph
//! so they line up with `widget_to_doc`: brush/eraser = circle center, pencil = tip, shapes = first corner, etc.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Total bitmap size (smaller cursors).
const CURSOR_PX: u32 = 24;
/// Inset so the icon glyph is visibly smaller than the bitmap.
const MARGIN: f32 = 4.0;

/// Lucide cursors use `stroke-width="2"` on the root; draw a wider white stroke underneath for contrast on black.
const CURSOR_OUTLINE_STROKE: &str = "5";

/// Duplicate each stroked shape with a white underlay (no filters).
fn inject_cursor_stroke_halo(svg: &str) -> String {
    let Some(svg_start) = svg.find("<svg") else {
        return svg.to_string();
    };
    let after_open = &svg[svg_start..];
    let Some(gt_rel) = after_open.find('>') else {
        return svg.to_string();
    };
    let body_start = svg_start + gt_rel + 1;
    let Some(close_pos) = svg.rfind("</svg>") else {
        return svg.to_string();
    };
    let body = &svg[body_start..close_pos];
    let new_body = svg_body_insert_stroke_outlines(body);
    let mut out = String::with_capacity(svg.len() + new_body.len());
    out.push_str(&svg[..body_start]);
    out.push_str(&new_body);
    out.push_str(&svg[close_pos..]);
    out
}

fn svg_body_insert_stroke_outlines(body: &str) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    let mut i = 0usize;
    while i < body.len() {
        let rest = &body[i..];
        if rest.starts_with("<path") {
            let stripped = &rest[5..];
            let idx = stripped
                .find("/>")
                .unwrap_or_else(|| panic!("cursor svg: expected path `/>` after byte {i}"));
            let inner = &stripped[..idx];
            let orig_len = 5 + idx + 2;
            out.push_str("  <path");
            out.push_str(inner);
            out.push_str(" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"");
            out.push_str(CURSOR_OUTLINE_STROKE);
            out.push_str("\" stroke-linecap=\"round\" stroke-linejoin=\"round\" />");
            out.push('\n');
            out.push_str(&rest[..orig_len]);
            i += orig_len;
        } else if rest.starts_with("<circle") {
            let stripped = &rest[7..];
            let idx = stripped
                .find("/>")
                .unwrap_or_else(|| panic!("cursor svg: expected circle `/>` after byte {i}"));
            let inner = &stripped[..idx];
            let orig_len = 7 + idx + 2;
            out.push_str("  <circle");
            out.push_str(inner);
            out.push_str(" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"");
            out.push_str(CURSOR_OUTLINE_STROKE);
            out.push_str("\" />");
            out.push('\n');
            out.push_str(&rest[..orig_len]);
            i += orig_len;
        } else if rest.starts_with("<rect") {
            let stripped = &rest[5..];
            let idx = stripped
                .find("/>")
                .unwrap_or_else(|| panic!("cursor svg: expected rect `/>` after byte {i}"));
            let inner = &stripped[..idx];
            let orig_len = 5 + idx + 2;
            out.push_str("  <rect");
            out.push_str(inner);
            out.push_str(" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"");
            out.push_str(CURSOR_OUTLINE_STROKE);
            out.push_str("\" />");
            out.push('\n');
            out.push_str(&rest[..orig_len]);
            i += orig_len;
        } else {
            match rest.find('<') {
                Some(0) => {
                    out.push('<');
                    i += 1;
                }
                Some(n) => {
                    out.push_str(&rest[..n]);
                    i += n;
                }
                None => {
                    out.push_str(rest);
                    i = body.len();
                }
            }
        }
    }
    out
}

fn rasterize(
    svg_path: &Path,
    png_path: &Path,
    hx_svg: f32,
    hy_svg: f32,
) -> Result<(i32, i32), String> {
    let raw = fs::read_to_string(svg_path).map_err(|e| format!("read {}: {e}", svg_path.display()))?;
    let svg = inject_cursor_stroke_halo(&raw);
    let data = svg.as_bytes();
    let tree = usvg::Tree::from_data(data, &usvg::Options::default())
        .map_err(|e| format!("parse {}: {e}", svg_path.display()))?;

    let w = tree.size().width() as f32;
    let h = tree.size().height() as f32;
    if w <= 0.0 || h <= 0.0 {
        return Err(format!("bad size {}", svg_path.display()));
    }
    let inner = CURSOR_PX as f32 - 2.0 * MARGIN;
    let scale = inner / w.max(h);
    let tx = MARGIN + (inner - w * scale) * 0.5;
    let ty = MARGIN + (inner - h * scale) * 0.5;
    let transform = tiny_skia::Transform::from_translate(tx, ty)
        .post_concat(tiny_skia::Transform::from_scale(scale, scale));

    let hx_pix_f = tx + hx_svg * scale;
    let hy_pix_f = ty + hy_svg * scale;
    let max = (CURSOR_PX - 1) as f32;
    let hx = hx_pix_f.round().clamp(0.0, max) as i32;
    let hy = hy_pix_f.round().clamp(0.0, max) as i32;

    let mut pixmap =
        tiny_skia::Pixmap::new(CURSOR_PX, CURSOR_PX).ok_or_else(|| "pixmap".to_string())?;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .save_png(png_path)
        .map_err(|e| format!("write {}: {e}", png_path.display()))?;
    Ok((hx, hy))
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let svg_dir = Path::new("assets/cursors/svg");

    // (rust stem / png name, svg file, hotspot in SVG user space matching Lucide 24×24 viewBox)
    let icons: &[(&str, &str, f32, f32)] = &[
        // Circle stamp center (matches `stamp_circle` around pointer).
        ("brush", "brush.svg", 10.2, 18.35),
        // Graphite tip (pencil nib).
        ("pixel", "pencil.svg", 3.85, 16.18),
        // Circle stamp center for eraser (same as brush).
        ("eraser", "eraser.svg", 12.0, 17.6),
        // Intake tip (path toward m2 22).
        ("eyedropper", "pipette.svg", 2.35, 21.75),
        // Pour / fill origin above drip.
        ("fill", "paint-bucket.svg", 10.5, 9.6),
        // First endpoint of the slash (matches first click as line start).
        ("line", "slash.svg", 2.15, 21.85),
        // Top-left of square stroke (matches first corner of axis-aligned rect).
        ("rect", "square.svg", 3.0, 3.0),
        // Top-left of circle’s bounding box (first corner of ellipse drag).
        ("ellipse", "circle.svg", 2.0, 2.0),
        // Top-left of dashed marquee.
        ("select", "square-dashed.svg", 3.0, 3.0),
        // Star / sparkles centroid (magic wand “active” point).
        ("wand", "wand.svg", 15.0, 9.0),
        ("move", "move.svg", 12.0, 12.0),
        // Palm / grab centroid for pan.
        ("hand", "hand.svg", 12.0, 14.0),
    ];

    let mut hs = String::from("// @generated by build.rs\n\n");

    println!("cargo:rerun-if-changed=build.rs");
    for (stem, file, hx_svg, hy_svg) in icons {
        let svg = svg_dir.join(file);
        println!("cargo:rerun-if-changed={}", svg.display());
        let png = out.join(format!("{stem}.png"));
        match rasterize(&svg, &png, *hx_svg, *hy_svg) {
            Ok((hx, hy)) => {
                hs.push_str(&format!(
                    "pub const HOTSPOT_{}: (i32, i32) = ({hx}, {hy});\n",
                    stem.to_ascii_uppercase()
                ));
            }
            Err(e) => panic!("cursor rasterize {stem}: {e}"),
        }
    }

    fs::write(out.join("cursor_hotspots.rs"), hs).expect("write cursor_hotspots.rs");
}
