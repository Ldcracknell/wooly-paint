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

/// Header / popover menu icons (square PNG — wide textures get scaled down by GTK and look tiny).
const MENU_ICON_PX: u32 = 32;
const MENU_ICON_MARGIN: f32 = 3.5;
/// Foreground on light Adwaita chrome.
const MENU_STROKE_LIGHT_UI: &str = "#241f31";
/// Foreground on dark Adwaita chrome.
const MENU_STROKE_DARK_UI: &str = "#f6f5f4";

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

fn rasterize_menu_svg(svg_path: &Path, png_path: &Path, stroke_hex: &str) -> Result<(), String> {
    let raw = fs::read_to_string(svg_path).map_err(|e| format!("read {}: {e}", svg_path.display()))?;
    let svg = raw.replace("currentColor", stroke_hex);
    let tree = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default())
        .map_err(|e| format!("parse {}: {e}", svg_path.display()))?;

    let w = tree.size().width() as f32;
    let h = tree.size().height() as f32;
    if w <= 0.0 || h <= 0.0 {
        return Err(format!("bad size {}", svg_path.display()));
    }
    let inner = MENU_ICON_PX as f32 - 2.0 * MENU_ICON_MARGIN;
    let scale = inner / w.max(h);
    let tx = MENU_ICON_MARGIN + (inner - w * scale) * 0.5;
    let ty = MENU_ICON_MARGIN + (inner - h * scale) * 0.5;
    let transform = tiny_skia::Transform::from_translate(tx, ty)
        .post_concat(tiny_skia::Transform::from_scale(scale, scale));

    let mut pixmap = tiny_skia::Pixmap::new(MENU_ICON_PX, MENU_ICON_PX)
        .ok_or_else(|| "menu pixmap".to_string())?;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .save_png(png_path)
        .map_err(|e| format!("write {}: {e}", png_path.display()))?;
    Ok(())
}

fn windows_target() -> bool {
    env::var_os("CARGO_CFG_TARGET_OS")
        .is_some_and(|v| v == "windows")
}

/// Embed `src/assets/icon.ico` so the `.exe` has a shell icon when pinned or when the app is not running.
fn embed_windows_exe_icon() {
    println!("cargo:rerun-if-changed=src/assets/icon.ico");
    let mut res = winres::WindowsResource::new();
    res.set_icon("src/assets/icon.ico");
    if let Err(e) = res.compile() {
        panic!(
            "Windows resource compile failed: {e}\n\
             Install the Windows SDK (rc.exe) or MinGW windres on PATH."
        );
    }
}

fn main() {
    if windows_target() {
        embed_windows_exe_icon();
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let svg_dir = Path::new("assets/cursors/svg");

    // (rust stem / png name, svg file, hotspot in SVG user space matching Lucide 24×24 viewBox)
    // Coordinates are tuned against the rasterized 24×24 PNG (margin + scale in `rasterize`), not
    // raw path math only — so the visible dab / tip / corner lines up with `widget_to_doc`.
    let icons: &[(&str, &str, f32, f32)] = &[
        // Center of the bristle blob in the bitmap (not handle geometry in SVG space).
        ("brush", "brush.svg", 3.0, 18.0),
        // Pencil nib tip (`floor` doc cell under the tip for `stamp_square`).
        ("pixel", "pencil.svg", 0.0, 19.5),
        // Eraser pad centroid in the raster.
        ("eraser", "eraser.svg", 10.5, 16.5),
        // Intake tip at path origin m2 22.
        ("eyedropper", "pipette.svg", 2.0, 22.0),
        // Pour / fill origin above drip.
        ("fill", "paint-bucket.svg", 10.5, 9.6),
        // Slash endpoint at (2,22) — first click anchors the line here.
        ("line", "slash.svg", 2.0, 22.0),
        // Outer top-left of the square stroke in the bitmap (~1px inset from centerline at (3,3)).
        ("rect", "square.svg", 2.0, 2.0),
        // Outer top-left of the circle stroke (fractional coords so 24px raster rounds to (4,4)).
        ("ellipse", "circle.svg", 0.6, 0.6),
        // Marquee corner aligned with rect cursor.
        ("select", "square-dashed.svg", 2.0, 2.0),
        // Tip of wand shaft (path `m3 21 9-9` → (12,12)), not the sparkles centroid.
        ("wand", "wand.svg", 12.0, 12.0),
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

    let menu_svg_dir = Path::new("assets/menu/svg");
    let menu_out = out.join("menu");
    fs::create_dir_all(&menu_out).expect("menu out dir");
    println!("cargo:rerun-if-changed=assets/menu/svg");

    let menu_icons: &[(&str, &str)] = &[
        ("file", "files.svg"),
        ("new", "file-plus.svg"),
        ("open", "folder-open.svg"),
        ("recent", "history.svg"),
        ("save", "save.svg"),
        ("save_as", "file-up.svg"),
        ("canvas", "image.svg"),
        ("resize", "maximize-2.svg"),
        ("flip_x", "flip-horizontal-2.svg"),
        ("flip_y", "flip-vertical-2.svg"),
        ("rotate", "rotate-cw.svg"),
        ("grid", "grid-3x3.svg"),
        ("settings", "settings.svg"),
        ("keybinds", "keyboard.svg"),
        ("updates", "updates.svg"),
        ("theme", "sun-moon.svg"),
        ("theme_default", "monitor.svg"),
        ("theme_light", "sun.svg"),
        ("theme_dark", "moon.svg"),
        ("palettes", "palette.svg"),
        ("import_hex", "file-down.svg"),
        ("export_hex", "file-up.svg"),
        ("manage_palettes", "library.svg"),
        ("image", "image.svg"),
    ];

    for (alias, file) in menu_icons {
        let svg = menu_svg_dir.join(file);
        println!("cargo:rerun-if-changed={}", svg.display());
        let light = menu_out.join(format!("{alias}_light.png"));
        let dark = menu_out.join(format!("{alias}_dark.png"));
        rasterize_menu_svg(&svg, &light, MENU_STROKE_LIGHT_UI)
            .unwrap_or_else(|e| panic!("menu icon {alias} light: {e}"));
        rasterize_menu_svg(&svg, &dark, MENU_STROKE_DARK_UI)
            .unwrap_or_else(|e| panic!("menu icon {alias} dark: {e}"));
    }
}
