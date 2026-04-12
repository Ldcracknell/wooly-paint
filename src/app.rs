use crate::document::{
    composite_layers_from_below_into, composite_layers_into, premul_rgba_to_cairo_argb32,
    premul_to_straight_rgba, premul_to_straight_rgba_into, straight_to_premul, Document,
};
use crate::palette::{self, PaletteBook};
use crate::state::{AppState, ColorSlot, FloatingDrag, FloatingSelection, Selection};
use crate::tool_cursors;
use crate::tools::{
    clear_rect, clear_region_masked, copy_rect, copy_region_masked, draw_ellipse, draw_rect_outline,
    ellipse_outline_segment_count, flood_fill, flood_select_mask, paste_rect, region_tight_bbox_or_hint,
    sample_composite_premul, stamp_circle, stamp_square, stroke_line, stroke_line_square, ToolKind,
};
use libadwaita::prelude::*;
use libadwaita::{Application, ColorScheme};
use gdk_pixbuf::Pixbuf;
use gtk::gdk;
use gtk::gdk::prelude::ToplevelExt;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::Cast;
use gtk::glib::ControlFlow;
#[allow(deprecated)]
use gtk::gio::prelude::ApplicationExt as GioApplicationExt;
use gtk::prelude::DrawingAreaExtManual;
use gtk::prelude::EventControllerExt;
use gtk::prelude::GestureDragExt;
use gtk::prelude::GestureSingleExt;
use gtk::prelude::RangeExt;
use gtk::prelude::EditableExt;
use gtk::prelude::ToggleButtonExt;
use gtk::prelude::WidgetExt;
use gtk::prelude::WidgetExtManual;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use gtk::prelude::ListItemExt;
use gtk::prelude::MenuModelExt;

type SharedState = Rc<RefCell<AppState>>;
type CanvasCell = Rc<RefCell<Option<gtk::DrawingArea>>>;
type LayersCell = Rc<RefCell<Option<gtk::ListBox>>>;
type PickerUiRefresh = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

fn picker_refresh_call(pr: &PickerUiRefresh) {
    if let Some(f) = pr.borrow().as_ref() {
        f();
    }
}
type ColorPreviewDaCell = Rc<RefCell<Option<gtk::DrawingArea>>>;
type ToolDdCell = Rc<RefCell<Option<gtk::DropDown>>>;
/// Screen-space size for floating handles (corners / edges / rotate knob), in pixels.
const FLOAT_HANDLE_PX: f64 = 7.0;
/// Rotation handle sits this many pixels beyond the top edge (document space; scaled by zoom in hit test via radius).
const FLOAT_ROT_OFFSET_DOC: f64 = 22.0;

fn floating_image_to_doc_matrix(f: &FloatingSelection) -> gtk::cairo::Matrix {
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    let cx = f.x + fw * 0.5;
    let cy = f.y + fh * 0.5;
    let sx = f.scale_x * if f.flip_h { -1.0 } else { 1.0 };
    let sy = f.scale_y * if f.flip_v { -1.0 } else { 1.0 };
    let mut m = gtk::cairo::Matrix::identity();
    m.translate(cx, cy);
    m.rotate(f.angle_deg.to_radians());
    m.scale(sx, sy);
    m.translate(-fw * 0.5, -fh * 0.5);
    m
}

fn floating_quad_doc(f: &FloatingSelection) -> [(f64, f64); 4] {
    let m = floating_image_to_doc_matrix(f);
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    [(0.0, 0.0), (fw, 0.0), (fw, fh), (0.0, fh)].map(|(px, py)| m.transform_point(px, py))
}

fn doc_point_in_floating(doc_x: f64, doc_y: f64, f: &FloatingSelection) -> bool {
    if f.scale_x <= 0.0 || f.scale_y <= 0.0 {
        return false;
    }
    let m = floating_image_to_doc_matrix(f);
    let inv = match m.try_invert() {
        Ok(i) => i,
        Err(_) => return false,
    };
    let (lx, ly) = inv.transform_point(doc_x, doc_y);
    lx >= -1e-3 && ly >= -1e-3 && lx <= f.w as f64 + 1e-3 && ly <= f.h as f64 + 1e-3
}

fn floating_transform_center(f: &FloatingSelection) -> (f64, f64) {
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    (f.x + fw * 0.5, f.y + fh * 0.5)
}

fn corner_local(i: u8, w: f64, h: f64) -> (f64, f64) {
    match i % 4 {
        0 => (0.0, 0.0),
        1 => (w, 0.0),
        2 => (w, h),
        _ => (0.0, h),
    }
}

/// Opposite edge's midpoint (local) used as fixed anchor when dragging edge `e`.
fn edge_anchor_local_for_resize(edge: u8, fw: f64, fh: f64) -> (f64, f64) {
    match edge % 4 {
        0 => (fw * 0.5, fh),
        1 => (0.0, fh * 0.5),
        2 => (fw * 0.5, 0.0),
        _ => (fw, fh * 0.5),
    }
}

fn rot_dist(angle_deg: f64, vx: f64, vy: f64) -> (f64, f64) {
    let mut mm = gtk::cairo::Matrix::identity();
    mm.rotate(angle_deg.to_radians());
    mm.transform_distance(vx, vy)
}

fn rot_dist_inv(angle_deg: f64, vx: f64, vy: f64) -> (f64, f64) {
    let mut mm = gtk::cairo::Matrix::identity();
    mm.rotate(-angle_deg.to_radians());
    mm.transform_distance(vx, vy)
}

fn dist_point(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

fn dist_seg(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let abx = bx - ax;
    let aby = by - ay;
    let apx = px - ax;
    let apy = py - ay;
    let ab2 = abx * abx + aby * aby;
    if ab2 < 1e-18 {
        return dist_point(p, a);
    }
    let t = ((apx * abx + apy * aby) / ab2).clamp(0.0, 1.0);
    let qx = ax + t * abx;
    let qy = ay + t * aby;
    dist_point(p, (qx, qy))
}

fn floating_handle_radius_doc(zoom: f64) -> f64 {
    (FLOAT_HANDLE_PX / zoom.max(0.001)).max(1.0)
}

fn floating_rotate_handle_doc(f: &FloatingSelection) -> (f64, f64) {
    let m = floating_image_to_doc_matrix(f);
    let fw = f.w.max(1) as f64;
    let top_mid = m.transform_point(fw * 0.5, 0.0);
    let c = floating_transform_center(f);
    let vx = top_mid.0 - c.0;
    let vy = top_mid.1 - c.1;
    let len = (vx * vx + vy * vy).sqrt();
    if len < 1e-6 {
        return (top_mid.0, top_mid.1 - FLOAT_ROT_OFFSET_DOC);
    }
    let ux = vx / len;
    let uy = vy / len;
    (
        top_mid.0 + ux * FLOAT_ROT_OFFSET_DOC,
        top_mid.1 + uy * FLOAT_ROT_OFFSET_DOC,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FloatPress {
    Outside,
    Rotate,
    Corner(u8),
    Edge(u8),
    Body,
}

fn float_press_at(doc_x: f64, doc_y: f64, zoom: f64, f: &FloatingSelection) -> FloatPress {
    let r = floating_handle_radius_doc(zoom);
    let hr = floating_rotate_handle_doc(f);
    if dist_point((doc_x, doc_y), hr) <= r * 1.45 {
        return FloatPress::Rotate;
    }
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    let m = floating_image_to_doc_matrix(f);
    for i in 0u8..4u8 {
        let (lx, ly) = corner_local(i, fw, fh);
        let p = m.transform_point(lx, ly);
        if dist_point((doc_x, doc_y), p) <= r {
            return FloatPress::Corner(i);
        }
    }
    for e in 0u8..4u8 {
        let (ax, ay) = corner_local(e, fw, fh);
        let (bx, by) = corner_local((e + 1) % 4, fw, fh);
        let pa = m.transform_point(ax, ay);
        let pb = m.transform_point(bx, by);
        if dist_seg((doc_x, doc_y), pa, pb) <= r {
            return FloatPress::Edge(e);
        }
    }
    if doc_point_in_floating(doc_x, doc_y, f) {
        FloatPress::Body
    } else {
        FloatPress::Outside
    }
}

fn apply_floating_resize_corner(
    f: &mut FloatingSelection,
    dragged_corner: u8,
    anchor_doc: (f64, f64),
    p_doc: (f64, f64),
) {
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    let opp = (dragged_corner + 2) % 4;
    let (ax, ay) = corner_local(opp, fw, fh);
    let (dx, dy) = corner_local(dragged_corner, fw, fh);
    let ux_a = ax - fw * 0.5;
    let uy_a = ay - fh * 0.5;
    let ux_d = dx - fw * 0.5;
    let uy_d = dy - fh * 0.5;
    let dux = ux_d - ux_a;
    let duy = uy_d - uy_a;
    let mut fh_s = if f.flip_h { -1.0 } else { 1.0 };
    let mut fv_s = if f.flip_v { -1.0 } else { 1.0 };
    let vx = p_doc.0 - anchor_doc.0;
    let vy = p_doc.1 - anchor_doc.1;
    let (wx, wy) = rot_dist_inv(f.angle_deg, vx, vy);
    let eps = 1e-6;
    let mut sx = f.scale_x;
    let mut sy = f.scale_y;
    if dux.abs() > eps {
        let mut raw = wx / (fh_s * dux);
        if raw < 0.0 {
            f.flip_h = !f.flip_h;
            fh_s = -fh_s;
            raw = -raw;
        }
        sx = raw.clamp(0.02, 100.0);
    }
    if duy.abs() > eps {
        let mut raw = wy / (fv_s * duy);
        if raw < 0.0 {
            f.flip_v = !f.flip_v;
            fv_s = -fv_s;
            raw = -raw;
        }
        sy = raw.clamp(0.02, 100.0);
    }
    let sxu_x = sx * fh_s * ux_a;
    let sxu_y = sy * fv_s * uy_a;
    let (rx, ry) = rot_dist(f.angle_deg, sxu_x, sxu_y);
    let cx = anchor_doc.0 - rx;
    let cy = anchor_doc.1 - ry;
    f.x = cx - fw * 0.5;
    f.y = cy - fh * 0.5;
    f.scale_x = sx;
    f.scale_y = sy;
}

fn apply_floating_resize_edge(
    f: &mut FloatingSelection,
    edge: u8,
    anchor_doc: (f64, f64),
    p_doc: (f64, f64),
) {
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    let mut fh_s = if f.flip_h { -1.0 } else { 1.0 };
    let mut fv_s = if f.flip_v { -1.0 } else { 1.0 };
    let vx = p_doc.0 - anchor_doc.0;
    let vy = p_doc.1 - anchor_doc.1;
    let (wx, wy) = rot_dist_inv(f.angle_deg, vx, vy);
    let eps = 1e-6;
    let mut sx = f.scale_x;
    let mut sy = f.scale_y;
    let (ux_a, uy_a) = match edge % 4 {
        0 => (0.0, fh * 0.5),
        1 => (-fw * 0.5, 0.0),
        2 => (0.0, -fh * 0.5),
        _ => (fw * 0.5, 0.0),
    };
    match edge % 4 {
        0 => {
            let mut raw = -wy / (fv_s * fh);
            if raw < 0.0 {
                f.flip_v = !f.flip_v;
                fv_s = -fv_s;
                raw = -raw;
            }
            sy = raw.clamp(0.02, 100.0);
        }
        1 => {
            if fw.abs() > eps {
                let mut raw = wx / (fh_s * fw);
                if raw < 0.0 {
                    f.flip_h = !f.flip_h;
                    fh_s = -fh_s;
                    raw = -raw;
                }
                sx = raw.clamp(0.02, 100.0);
            }
        }
        2 => {
            let mut raw = wy / (fv_s * fh);
            if raw < 0.0 {
                f.flip_v = !f.flip_v;
                fv_s = -fv_s;
                raw = -raw;
            }
            sy = raw.clamp(0.02, 100.0);
        }
        _ => {
            if fw.abs() > eps {
                let mut raw = wx / (fh_s * (-fw));
                if raw < 0.0 {
                    f.flip_h = !f.flip_h;
                    fh_s = -fh_s;
                    raw = -raw;
                }
                sx = raw.clamp(0.02, 100.0);
            }
        }
    }
    let sxu_x = sx * fh_s * ux_a;
    let sxu_y = sy * fv_s * uy_a;
    let (rx, ry) = rot_dist(f.angle_deg, sxu_x, sxu_y);
    let cx = anchor_doc.0 - rx;
    let cy = anchor_doc.1 - ry;
    f.x = cx - fw * 0.5;
    f.y = cy - fh * 0.5;
    f.scale_x = sx;
    f.scale_y = sy;
}

fn draw_floating_handles(cr: &gtk::cairo::Context, zoom: f64, f: &FloatingSelection) {
    let fw = f.w.max(1) as f64;
    let fh = f.h.max(1) as f64;
    let m = floating_image_to_doc_matrix(f);
    let hs = (4.5_f64 / zoom.max(0.001)).max(1.5);
    let draw_sq = |cr: &gtk::cairo::Context, cx: f64, cy: f64| {
        cr.rectangle(cx - hs, cy - hs, hs * 2.0, hs * 2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.fill_preserve().unwrap();
        cr.set_source_rgba(0.1, 0.1, 0.1, 0.95);
        cr.set_line_width(1.0 / zoom.max(0.001));
        cr.stroke().unwrap();
    };
    for i in 0u8..4u8 {
        let (lx, ly) = corner_local(i, fw, fh);
        let p = m.transform_point(lx, ly);
        draw_sq(cr, p.0, p.1);
    }
    for e in 0u8..4u8 {
        let (ax, ay) = corner_local(e, fw, fh);
        let (bx, by) = corner_local((e + 1) % 4, fw, fh);
        let pa = m.transform_point(ax, ay);
        let pb = m.transform_point(bx, by);
        let mx = (pa.0 + pb.0) * 0.5;
        let my = (pa.1 + pb.1) * 0.5;
        draw_sq(cr, mx, my);
    }
    let rh = floating_rotate_handle_doc(f);
    cr.arc(rh.0, rh.1, hs * 1.05, 0.0, std::f64::consts::TAU);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.fill_preserve().unwrap();
    cr.set_source_rgba(0.1, 0.1, 0.1, 0.95);
    cr.set_line_width(1.0 / zoom.max(0.001));
    cr.stroke().unwrap();
}

/// Cairo ARgb32 row (BGRA premultiplied) → tight premultiplied RGBA for layers.
fn cairo_stride_to_premul_rgba_tight(src: &[u8], w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let s0 = row * stride;
        let d0 = row * w * 4;
        for col in 0..w {
            let s = s0 + col * 4;
            let d = d0 + col * 4;
            out[d] = src[s + 2];
            out[d + 1] = src[s + 1];
            out[d + 2] = src[s];
            out[d + 3] = src[s + 3];
        }
    }
    out
}

fn rasterize_floating_to_premul(f: &FloatingSelection) -> Option<(i32, i32, i32, i32, Vec<u8>)> {
    if f.scale_x <= 0.0 || f.scale_y <= 0.0 {
        return None;
    }
    let pts = floating_quad_doc(f);
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (dx, dy) in pts {
        min_x = min_x.min(dx);
        min_y = min_y.min(dy);
        max_x = max_x.max(dx);
        max_y = max_y.max(dy);
    }
    let min_ix = min_x.floor() as i32;
    let min_iy = min_y.floor() as i32;
    let max_ix = max_x.ceil() as i32;
    let max_iy = max_y.ceil() as i32;
    let rw = (max_ix - min_ix).max(1);
    let rh = (max_iy - min_iy).max(1);
    if rw > 8192 || rh > 8192 {
        return None;
    }
    let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, rw, rh).ok()?;
    {
        let cr = gtk::cairo::Context::new(&surf).ok()?;
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.set_operator(gtk::cairo::Operator::Clear);
        cr.paint().ok()?;
               cr.set_operator(gtk::cairo::Operator::Over);
        let fw = f.w.max(1);
        let fh = f.h.max(1);
        let mut src_surf =
            gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, fw, fh).ok()?;
        {
            let stride = src_surf.stride() as usize;
            let mut data = src_surf.data().ok()?;
            premul_rgba_to_cairo_argb32(
                &mut data,
                stride,
                fw as u32,
                fh as u32,
                &f.data,
            );
        }
        let m = floating_image_to_doc_matrix(f);
        cr.translate(-(min_ix as f64), -(min_iy as f64));
        cr.transform(m);
        cr.set_source_surface(&src_surf, 0.0, 0.0).ok()?;
        cr.source().set_filter(gtk::cairo::Filter::Nearest);
        cr.paint().ok()?;
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let w = surf.width() as usize;
    let h = surf.height() as usize;
    let owned = surf.take_data().ok()?;
    let raw: &[u8] = owned.as_ref();
    let premul = cairo_stride_to_premul_rgba_tight(raw, w, h, stride);
    Some((min_ix, min_iy, rw, rh, premul))
}

const APP_APPLICATION_ID: &str = "dev.woolymelon.WoolyPaint";

/// Windows groups the taskbar button and pinned shortcut by AppUserModelID. Without this, the
/// shell often falls back to a generic identity and the pinned icon vanishes when the app exits.
#[cfg(windows)]
fn set_windows_app_user_model_id(app_id: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(appid: *const u16) -> i32;
    }

    let mut wide: Vec<u16> = OsStr::new(app_id).encode_wide().collect();
    wide.push(0);
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(wide.as_ptr());
    }
}

pub fn run() -> gtk::glib::ExitCode {
    #[cfg(windows)]
    set_windows_app_user_model_id(APP_APPLICATION_ID);

    libadwaita::init().expect("libadwaita init");
    apply_libadwaita_theme_from_disk_early();
    let app = Application::builder()
        .application_id(APP_APPLICATION_ID)
        .build();

    app.connect_activate(build_ui);
    app.run()
}

/// Apply saved [`AdwStyleManager`](libadwaita::StyleManager) colors before `GtkApplication` exists,
/// and clear GTK's legacy `gtk-application-prefer-dark-theme` for this process. User `settings.ini`
/// often sets that flag; combining it with libadwaita triggers:
/// "Using GtkSettings:gtk-application-prefer-dark-theme with libadwaita is unsupported".
fn apply_libadwaita_theme_from_disk_early() {
    let menu = crate::settings::saved_color_scheme_menu_value();
    libadwaita::StyleManager::default().set_color_scheme(color_scheme_from_menu_value(menu));
    if let Some(display) = gdk::Display::default() {
        gtk::Settings::for_display(&display).set_gtk_application_prefer_dark_theme(false);
    }
}

fn tool_label(tool: ToolKind, key: Option<char>) -> String {
    match key {
        Some(c) => format!("{} ({})", tool.display_name(), c.to_ascii_uppercase()),
        None => tool.display_name().to_string(),
    }
}

/// Minimum width for the tool [`gtk::DropDown`] so every option fits at the widget font without truncation.
fn tool_dropdown_width_request(dropdown: &gtk::DropDown, labels: &[String]) -> i32 {
    const DROPDOWN_CHROME_PX: i32 = 52;
    let mut max_label_px = 0i32;
    for text in labels {
        let layout = dropdown.create_pango_layout(Some(text.as_str()));
        let (w, _) = layout.pixel_size();
        max_label_px = max_label_px.max(w);
    }
    max_label_px + DROPDOWN_CHROME_PX
}

fn refresh_tool_labels(state: &SharedState, sl: &gtk::StringList) {
    let labels: Vec<String> = {
        let st = state.borrow();
        st.tool_keybinds.iter().map(|(t, k)| tool_label(*t, *k)).collect()
    };
    let strs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    sl.splice(0, sl.n_items(), &strs);
}

fn queue_canvas(canvas: &CanvasCell) {
    if let Some(ref c) = *canvas.borrow() {
        c.queue_draw();
    }
}

/// App icon from `src/assets/icon.png` (embedded at compile time).
fn embedded_app_icon_texture() -> Option<gdk::Texture> {
    static ICON_PNG: &[u8] = include_bytes!("assets/icon.png");
    let bytes = glib::Bytes::from_static(ICON_PNG);
    gdk::Texture::from_bytes(&bytes).ok()
}

fn apply_taskbar_icon(native: &impl gtk::prelude::IsA<gtk::Native>) {
    let Some(texture) = embedded_app_icon_texture() else {
        return;
    };
    let Some(surface) = native.surface() else {
        return;
    };
    let Some(toplevel) = surface.dynamic_cast_ref::<gdk::Toplevel>() else {
        return;
    };
    toplevel.set_icon_list(std::slice::from_ref(&texture));
}

fn rgb_bytes_to_hsv(c: [u8; 4]) -> (f64, f64, f64) {
    let r = c[0] as f64 / 255.0;
    let g = c[1] as f64 / 255.0;
    let b = c[2] as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 1e-9 {
        0.0
    } else if (max - r).abs() <= 1e-9 {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() <= 1e-9 {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 1e-9 { 0.0 } else { d / max };
    (h.rem_euclid(1.0), s, max)
}

/// (s, v) on the `hh` slice that best matches RGB `c` (for SV square marker position).
fn sv_on_hue_plane_for_rgb(hh: f64, c: [u8; 4]) -> (f64, f64) {
    let tr = c[0] as f64 / 255.0;
    let tg = c[1] as f64 / 255.0;
    let tb = c[2] as f64 / 255.0;
    let mut best_s = 0.0_f64;
    let mut best_v = 0.0_f64;
    let mut best_e = f64::MAX;
    const N: i32 = 40;
    for i in 0..=N {
        let s = i as f64 / N as f64;
        for j in 0..=N {
            let v = j as f64 / N as f64;
            let (r, g, b) = hsv_to_rgb01(hh, s, v);
            let e = (r - tr).powi(2) + (g - tg).powi(2) + (b - tb).powi(2);
            if e < best_e {
                best_e = e;
                best_s = s;
                best_v = v;
            }
        }
    }
    (best_s, best_v)
}

fn hsv_to_rgb01(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let h = h.rem_euclid(1.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

fn u8_chan(x: f64) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn write_slot_rgb_a(st: &mut AppState, slot: ColorSlot, rgb: (f64, f64, f64), a: u8) {
    let (r, g, b) = rgb;
    let c = match slot {
        ColorSlot::Left => &mut st.fg,
        ColorSlot::Right => &mut st.bg,
    };
    *c = [u8_chan(r), u8_chan(g), u8_chan(b), a];
}

fn write_slot_rgb(st: &mut AppState, slot: ColorSlot, rgb: (f64, f64, f64)) {
    let a = match slot {
        ColorSlot::Left => st.fg[3],
        ColorSlot::Right => st.bg[3],
    };
    write_slot_rgb_a(st, slot, rgb, a);
}

/// Picker gestures: left (and other non-right) buttons edit the primary slot; right button edits secondary.
fn picker_target_for_gesture_button(btn: u32) -> ColorSlot {
    if btn == gdk::BUTTON_SECONDARY {
        ColorSlot::Right
    } else {
        ColorSlot::Left
    }
}

fn apply_sv_pick(
    state: &SharedState,
    sv_area: &gtk::DrawingArea,
    fg_bg: &gtk::DrawingArea,
    canvas: &CanvasCell,
    picker_refresh: &PickerUiRefresh,
    x: f64,
    y: f64,
) {
    let w = sv_area.width() as f64;
    let h = sv_area.height() as f64;
    if w <= 1.0 || h <= 1.0 {
        return;
    }
    let s = (x / w).clamp(0.0, 1.0);
    let v = 1.0 - (y / h).clamp(0.0, 1.0);
    let mut g = state.borrow_mut();
    let hh = g.picker_hue;
    let slot = g.picker_target;
    let rgb = hsv_to_rgb01(hh, s, v);
    write_slot_rgb(&mut g, slot, rgb);
    drop(g);
    picker_refresh_call(picker_refresh);
    sv_area.queue_draw();
    fg_bg.queue_draw();
    queue_canvas(canvas);
}

fn sync_picker_from_target_color(
    state: &SharedState,
    hue_sup: &Cell<bool>,
    picker_sup: &Cell<bool>,
    hue_adj: &gtk::Adjustment,
    sat_adj: &gtk::Adjustment,
    val_adj: &gtk::Adjustment,
    r_adj: &gtk::Adjustment,
    g_adj: &gtk::Adjustment,
    b_adj: &gtk::Adjustment,
    a_adj: &gtk::Adjustment,
    sat_disp_adj: &gtk::Adjustment,
    val_disp_adj: &gtk::Adjustment,
    hex_entry: &gtk::Entry,
    picker_tracks: &Rc<RefCell<Vec<gtk::DrawingArea>>>,
    sv_square: &gtk::DrawingArea,
) {
    if picker_sup.get() {
        return;
    }
    picker_sup.set(true);
    let c = {
        let st = state.borrow();
        match st.picker_target {
            ColorSlot::Left => st.fg,
            ColorSlot::Right => st.bg,
        }
    };
    let (h_rgb, s_rgb, v_rgb) = rgb_bytes_to_hsv(c);
    let v_clamped = v_rgb.clamp(0.0, 1.0);
    // Black (V=0) and grayscale (S=0) don't determine H (and black doesn't determine S) in RGB.
    // Keep the slider/model hue and saturation so H/S stay draggable off black and grays stay tinted.
    let h_sync = if v_clamped <= 1e-9 {
        state.borrow().picker_hue.rem_euclid(1.0)
    } else if s_rgb <= 1e-9 {
        state.borrow().picker_hue.rem_euclid(1.0)
    } else {
        h_rgb.rem_euclid(1.0)
    };
    let s_sync = if v_clamped <= 1e-9 {
        sat_adj.value().clamp(0.0, 1.0)
    } else {
        s_rgb.clamp(0.0, 1.0)
    };
    state.borrow_mut().picker_hue = h_sync;
    hue_sup.set(true);
    hue_adj.set_value(h_sync * 360.0);
    hue_sup.set(false);
    sat_adj.set_value(s_sync);
    val_adj.set_value(v_clamped);
    r_adj.set_value(c[0] as f64);
    g_adj.set_value(c[1] as f64);
    b_adj.set_value(c[2] as f64);
    a_adj.set_value(c[3] as f64);
    sat_disp_adj.set_value((s_sync * 100.0).round().clamp(0.0, 100.0));
    val_disp_adj.set_value((v_clamped * 100.0).round().clamp(0.0, 100.0));
    let hex = if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            c[0], c[1], c[2], c[3]
        )
    };
    hex_entry.set_text(&hex);
    picker_sup.set(false);
    for d in picker_tracks.borrow().iter() {
        d.queue_draw();
    }
    sv_square.queue_draw();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickerTrackKind {
    Hue,
    Sat,
    Val,
    Red,
    Green,
    Blue,
    Alpha,
}

fn draw_checker_bg(cr: &gtk::cairo::Context, width: f64, height: f64) {
    let sz = 5.0_f64;
    let mut y = 0.0;
    while y < height {
        let mut x = 0.0;
        while x < width {
            let ix = (x / sz).floor() as i32;
            let iy = (y / sz).floor() as i32;
            let light = (ix + iy).rem_euclid(2) == 0;
            if light {
                cr.set_source_rgb(0.62, 0.62, 0.62);
            } else {
                cr.set_source_rgb(0.38, 0.38, 0.38);
            }
            let rw = sz.min(width - x);
            let rh = sz.min(height - y);
            cr.rectangle(x, y, rw, rh);
            let _ = cr.fill();
            x += sz;
        }
        y += sz;
    }
}

fn make_picker_track(
    kind: PickerTrackKind,
    state: &SharedState,
    adj: &gtk::Adjustment,
    picker_tracks: &Rc<RefCell<Vec<gtk::DrawingArea>>>,
) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::builder()
        .height_request(22)
        .hexpand(true)
        .build();
    let st = state.clone();
    let adj_c = adj.clone();
    da.set_draw_func(move |_d, cr, ww, hh| {
        let w = ww as f64;
        let h = hh as f64;
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        let st_b = st.borrow();
        let c = match st_b.picker_target {
            ColorSlot::Left => st_b.fg,
            ColorSlot::Right => st_b.bg,
        };
        let hh_pick = st_b.picker_hue;
        let (_, s0, v0) = rgb_bytes_to_hsv(c);
        let r0 = c[0] as f64 / 255.0;
        let g0 = c[1] as f64 / 255.0;
        let b0 = c[2] as f64 / 255.0;
        drop(st_b);
        if kind != PickerTrackKind::Alpha {
            let ww_i = ww.max(1);
            for px in 0..ww_i {
                let t = (px as f64 + 0.5) / w;
                let (r, g, b) = match kind {
                    PickerTrackKind::Hue => hsv_to_rgb01(t, 1.0, 1.0),
                    PickerTrackKind::Sat => hsv_to_rgb01(hh_pick, t, v0),
                    PickerTrackKind::Val => hsv_to_rgb01(hh_pick, s0, t),
                    PickerTrackKind::Red => (t, g0, b0),
                    PickerTrackKind::Green => (r0, t, b0),
                    PickerTrackKind::Blue => (r0, g0, t),
                    PickerTrackKind::Alpha => (0.0, 0.0, 0.0),
                };
                cr.set_source_rgb(r, g, b);
                cr.rectangle(px as f64, 0.0, 1.0, h);
                let _ = cr.fill();
            }
        } else {
            draw_checker_bg(cr, w, h);
            let lg = gtk::cairo::LinearGradient::new(0.0, 0.0, w, 0.0);
            lg.add_color_stop_rgba(0.0, r0, g0, b0, 0.0);
            lg.add_color_stop_rgba(1.0, r0, g0, b0, 1.0);
            let _ = cr.set_source(&lg);
            cr.rectangle(0.0, 0.0, w, h);
            let _ = cr.fill();
        }
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
        cr.set_line_width(1.0);
        cr.rectangle(0.5, 0.5, w - 1.0, h - 1.0);
        let _ = cr.stroke();
        let lo = adj_c.lower();
        let hi = adj_c.upper();
        let t = if (hi - lo).abs() < 1e-9 {
            0.0
        } else {
            ((adj_c.value() - lo) / (hi - lo)).clamp(0.0, 1.0)
        };
        let x = t * w;
        cr.set_source_rgb(0.06, 0.06, 0.06);
        cr.move_to(x, 1.0);
        cr.line_to(x - 5.0, h - 1.0);
        cr.line_to(x + 5.0, h - 1.0);
        cr.close_path();
        let _ = cr.fill();
    });
    picker_tracks.borrow_mut().push(da.clone());
    wire_picker_track_drag(&da, adj, picker_tracks, state);
    da
}

fn wire_picker_track_drag(
    da: &gtk::DrawingArea,
    adj: &gtk::Adjustment,
    picker_tracks: &Rc<RefCell<Vec<gtk::DrawingArea>>>,
    state: &SharedState,
) {
    let painting = Rc::new(Cell::new(false));
    let gc = gtk::GestureClick::new();
    gc.set_button(0);
    let adj_p = adj.clone();
    let da_p = da.clone();
    let tracks_p = picker_tracks.clone();
    let paint_on = painting.clone();
    let st_press = state.clone();
    gc.connect_pressed(move |gesture, _, x, _| {
        paint_on.set(true);
        {
            let mut g = st_press.borrow_mut();
            g.picker_target = picker_target_for_gesture_button(gesture.current_button());
        }
        let w = da_p.width() as f64;
        if w > 1.0 {
            let t = (x / w).clamp(0.0, 1.0);
            let lo = adj_p.lower();
            let hi = adj_p.upper();
            let mut v = lo + t * (hi - lo);
            if adj_p.step_increment() >= 1.0 && hi > 1.5 {
                v = v.round();
            }
            adj_p.set_value(v.clamp(lo, hi));
        }
        for tr in tracks_p.borrow().iter() {
            tr.queue_draw();
        }
    });
    let paint_off = painting.clone();
    gc.connect_released(move |_, _, _, _| paint_off.set(false));
    da.add_controller(gc);

    let motion = gtk::EventControllerMotion::new();
    let adj_m = adj.clone();
    let da_m = da.clone();
    let tracks_m = picker_tracks.clone();
    let painting_m = painting.clone();
    motion.connect_motion(move |_ec, x, _| {
        if !painting_m.get() {
            return;
        }
        let w = da_m.width() as f64;
        if w > 1.0 {
            let t = (x / w).clamp(0.0, 1.0);
            let lo = adj_m.lower();
            let hi = adj_m.upper();
            let mut v = lo + t * (hi - lo);
            if adj_m.step_increment() >= 1.0 && hi > 1.5 {
                v = v.round();
            }
            adj_m.set_value(v.clamp(lo, hi));
        }
        for tr in tracks_m.borrow().iter() {
            tr.queue_draw();
        }
    });
    da.add_controller(motion);
}

fn picker_gradient_spin_row(label: &str, track: &gtk::DrawingArea, spin: &gtk::SpinButton) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .hexpand(true)
        .build();
    let l = gtk::Label::new(Some(label));
    l.set_width_request(28);
    l.set_xalign(1.0);
    l.add_css_class("dim-label");
    spin.set_width_request(72);
    spin.set_digits(0);
    row.append(&l);
    track.set_hexpand(true);
    row.append(track);
    row.append(spin);
    row
}

fn push_recent_color(st: &mut AppState, fg: [u8; 4]) {
    st.recent_colors.retain(|c| *c != fg);
    st.recent_colors.insert(0, fg);
    st.recent_colors.truncate(4);
}

/// `fill_cell`: use for palette grids — color fills the whole tile and grows with sidebar width.
fn swatch_button(fg: [u8; 4], fill_cell: bool) -> gtk::Button {
    let da = if fill_cell {
        gtk::DrawingArea::builder()
            .width_request(20)
            .height_request(26)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .build()
    } else {
        gtk::DrawingArea::builder()
            .width_request(22)
            .height_request(22)
            .hexpand(false)
            .vexpand(false)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build()
    };
    let r = fg[0] as f64 / 255.0;
    let g = fg[1] as f64 / 255.0;
    let b = fg[2] as f64 / 255.0;
    let a = fg[3] as f64 / 255.0;
    da.set_draw_func(move |_d, cr, w, h| {
        cr.set_source_rgba(r, g, b, a);
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        let _ = cr.fill();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
        cr.set_line_width(1.0);
        cr.rectangle(0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0);
        let _ = cr.stroke();
    });
    let btn = if fill_cell {
        gtk::Button::builder()
            .child(&da)
            .css_classes(["flat"])
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .build()
    } else {
        gtk::Button::builder()
            .child(&da)
            .css_classes(["flat"])
            .hexpand(false)
            .vexpand(false)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build()
    };
    btn.set_tooltip_text(Some(&format!("RGB {}, {}, {}", fg[0], fg[1], fg[2])));
    btn
}

/// Square tile matching palette fill swatches: neutral field, border, centered “+”.
fn palette_add_color_tile_button() -> gtk::Button {
    let da = gtk::DrawingArea::builder()
        .width_request(20)
        .height_request(26)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Fill)
        .build();
    da.set_draw_func(move |_d, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        if w < 1.0 || h < 1.0 {
            return;
        }
        cr.set_source_rgba(0.12, 0.12, 0.13, 1.0);
        cr.rectangle(0.0, 0.0, w, h);
        let _ = cr.fill();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
        cr.set_line_width(1.0);
        cr.rectangle(0.5, 0.5, w - 1.0, h - 1.0);
        let _ = cr.stroke();
        let cx = w * 0.5;
        let cy = h * 0.5;
        let hl = (w.min(h) * 0.22).clamp(4.0, 9.0);
        cr.set_source_rgba(0.78, 0.78, 0.82, 0.92);
        cr.set_line_width(1.25);
        cr.move_to(cx - hl, cy);
        cr.line_to(cx + hl, cy);
        cr.move_to(cx, cy - hl);
        cr.line_to(cx, cy + hl);
        let _ = cr.stroke();
    });
    gtk::Button::builder()
        .child(&da)
        .css_classes(["flat"])
        .hexpand(true)
        .vexpand(true)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Fill)
        .build()
}

#[derive(Clone)]
struct PaletteSidebar {
    flow: gtk::FlowBox,
    dropdown: gtk::DropDown,
    strings: gtk::StringList,
    preview: gtk::DrawingArea,
    recent: gtk::FlowBox,
    canvas: CanvasCell,
    sv_area: gtk::DrawingArea,
    picker_refresh: PickerUiRefresh,
}

fn palette_dropdown_row_factory() -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    f.connect_setup(|_, item| {
        let Some(li) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let lbl = gtk::Label::builder().xalign(0.0).build();
        li.set_child(Some(&lbl));
    });
    f.connect_bind(|_, item| {
        let Some(li) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(lbl) = li.child().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(obj) = li.item() else {
            return;
        };
        let Ok(so) = obj.downcast::<gtk::StringObject>() else {
            return;
        };
        let s = so.string();
        let is_new_row = s.as_str() == crate::palette::NEW_PALETTE_DROPDOWN_LABEL;
        lbl.set_text(&s);
        if is_new_row {
            lbl.add_css_class("dim-label");
        } else {
            lbl.remove_css_class("dim-label");
        }
    });
    f
}

fn sync_palette_dropdown_model(book: &PaletteBook, strings: &gtk::StringList, dd: &gtk::DropDown) {
    let mut labels: Vec<&str> = book.entries.iter().map(|e| e.name.as_str()).collect();
    labels.push(crate::palette::NEW_PALETTE_DROPDOWN_LABEL);
    strings.splice(0, strings.n_items(), &labels);
    let sel = book.active.min(book.entries.len().saturating_sub(1));
    dd.set_selected(sel as u32);
}

fn fill_palette_swatches(ps: &PaletteSidebar, state: &SharedState) {
    let flow = &ps.flow;
    while let Some(c) = flow.first_child() {
        flow.remove(&c);
    }
    let colors: Vec<[u8; 4]> = state.borrow().palette_book.active_colors().to_vec();
    for &col in &colors {
        let btn = swatch_button(col, true);
        let st_pal = state.clone();
        let prev_pal = ps.preview.clone();
        let cv_pal = ps.canvas.clone();
        let rf_pal = ps.recent.clone();
        let sv = ps.sv_area.clone();
        let pr = ps.picker_refresh.clone();
        let gc = gtk::GestureClick::new();
        gc.set_button(0);
        gc.connect_pressed(move |gesture, _, _, _| {
            let btn_id = gesture.current_button();
            {
                let mut g = st_pal.borrow_mut();
                if btn_id == gdk::BUTTON_SECONDARY {
                    g.bg = col;
                    g.picker_target = ColorSlot::Right;
                } else {
                    g.fg = col;
                    g.picker_target = ColorSlot::Left;
                    push_recent_color(&mut g, col);
                }
            }
            picker_refresh_call(&pr);
            prev_pal.queue_draw();
            sv.queue_draw();
            queue_canvas(&cv_pal);
            refresh_recent_swatch_row(&rf_pal, &st_pal, &prev_pal, &cv_pal, &pr);
        });
        btn.add_controller(gc);
        flow.append(&btn);
    }

    let add_btn = palette_add_color_tile_button();
    add_btn.set_tooltip_text(Some(
        "Add the picker’s current colour (highlighted square) to this palette.",
    ));
    let st_add = state.clone();
    let ps_add = ps.clone();
    add_btn.connect_clicked(move |_| {
        let rgba = {
            let g = st_add.borrow();
            match g.picker_target {
                ColorSlot::Left => g.fg,
                ColorSlot::Right => g.bg,
            }
        };
        let added = {
            let mut g = st_add.borrow_mut();
            let ok = g.palette_book.append_color_to_active(rgba);
            if ok {
                crate::settings::persist(&g);
            }
            ok
        };
        if added {
            fill_palette_swatches(&ps_add, &st_add);
        }
    });
    flow.append(&add_btn);
}

fn refresh_palette_sidebar_full(ps: &PaletteSidebar, state: &SharedState) {
    let book = state.borrow().palette_book.clone();
    sync_palette_dropdown_model(&book, &ps.strings, &ps.dropdown);
    fill_palette_swatches(ps, state);
}

/// GTK file dialogs default to the process working directory; on Windows that is often
/// `System32` when the app is launched from a shortcut. Prefer the document folder,
/// a recent file's folder, then the standard Documents directory.
fn file_dialog_initial_folder(doc_path: Option<&Path>, recent_files: &[PathBuf]) -> gio::File {
    let try_parent = |p: &Path| -> Option<gio::File> {
        let parent = p.parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        Some(gio::File::for_path(parent))
    };

    if let Some(p) = doc_path {
        if let Some(f) = try_parent(p) {
            return f;
        }
    }
    for recent in recent_files {
        if let Some(f) = try_parent(recent) {
            return f;
        }
    }
    if let Some(d) = glib::user_special_dir(glib::UserDirectory::Documents) {
        return gio::File::for_path(d);
    }
    if let Some(h) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    {
        return gio::File::for_path(h);
    }
    gio::File::for_path(".")
}

fn sanitize_palette_filename_base(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "palette".to_string()
    } else {
        s
    }
}

fn import_palette_file(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    ps: &PaletteSidebar,
) {
    let text_filter = gtk::FileFilter::new();
    text_filter.set_name(Some("Hex palette"));
    for pat in ["*.hex", "*.txt", "*.pal"] {
        text_filter.add_pattern(pat);
    }
    let all_filter = gtk::FileFilter::new();
    all_filter.set_name(Some("All files"));
    all_filter.add_pattern("*");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&text_filter);
    filters.append(&all_filter);
    let initial_folder = {
        let g = state.borrow();
        file_dialog_initial_folder(g.doc.path.as_deref(), &g.recent_files)
    };
    let dlg = gtk::FileDialog::builder()
        .title("Import palette")
        .modal(true)
        .filters(&filters)
        .default_filter(&text_filter)
        .initial_folder(&initial_folder)
        .build();
    let st = state.clone();
    let psc = ps.clone();
    let w_err = window.clone();
    dlg.open(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            let Some(path) = file.path() else {
                return;
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Imported")
                .to_string();
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Read failed: {e}");
                    return;
                }
            };
            let text = String::from_utf8_lossy(&bytes);
            let colors = match palette::parse_hex_palette_text(&text) {
                Ok(c) => c,
                Err(msg) => {
                    let w = w_err.clone();
                    glib::idle_add_local_once(move || {
                        show_simple_alert(&w, "Import failed", &msg);
                    });
                    return;
                }
            };
            glib::idle_add_local_once(move || {
                {
                    let mut g = st.borrow_mut();
                    g.palette_book.push_palette(stem, colors);
                }
                crate::settings::persist(&st.borrow());
                refresh_palette_sidebar_full(&psc, &st);
            });
        }
    });
}

fn export_palette_file(window: &libadwaita::ApplicationWindow, state: &SharedState) {
    let (default_name, body) = {
        let g = state.borrow();
        let name = g.palette_book.active_palette().name.clone();
        let body = palette::format_hex_palette(g.palette_book.active_colors());
        (name, body)
    };
    let text_filter = gtk::FileFilter::new();
    text_filter.set_name(Some("Hex text (*.hex)"));
    text_filter.add_pattern("*.hex");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&text_filter);
    let initial = format!("{}.hex", sanitize_palette_filename_base(&default_name));
    let initial_folder = {
        let g = state.borrow();
        file_dialog_initial_folder(g.doc.path.as_deref(), &g.recent_files)
    };
    let dlg = gtk::FileDialog::builder()
        .title("Export palette")
        .modal(true)
        .filters(&filters)
        .default_filter(&text_filter)
        .initial_folder(&initial_folder)
        .initial_name(&initial)
        .build();
    let w_alert = window.clone();
    dlg.save(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            let Some(mut path) = file.path() else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("hex");
            }
            if let Err(e) = std::fs::write(&path, body.as_str()) {
                let w = w_alert.clone();
                let es = e.to_string();
                glib::idle_add_local_once(move || {
                    show_simple_alert(&w, "Export failed", &es);
                });
            }
        }
    });
}

fn manage_palettes_dialog(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    ps: &PaletteSidebar,
) {
    let d = libadwaita::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Manage palettes")
        .default_width(380)
        .default_height(440)
        .resizable(true)
        .build();

    let strings = gtk::StringList::new(&[]);
    let dd = gtk::DropDown::new(Some(strings.clone()), gtk::Expression::NONE);
    dd.set_hexpand(true);

    let rename_entry = gtk::Entry::builder()
        .placeholder_text("New name")
        .hexpand(true)
        .build();

    let sync_dd = {
        let st = state.clone();
        let strings_c = strings.clone();
        let dd_c = dd.clone();
        Rc::new(move || {
            let book = st.borrow().palette_book.clone();
            let names: Vec<&str> = book.entries.iter().map(|e| e.name.as_str()).collect();
            strings_c.splice(0, strings_c.n_items(), &names);
            let sel = book.active.min(book.entries.len().saturating_sub(1));
            dd_c.set_selected(sel as u32);
        })
    };

    let refresh_main = {
        let st = state.clone();
        let psc = ps.clone();
        Rc::new(move || {
            refresh_palette_sidebar_full(&psc, &st);
        })
    };

    sync_dd();

    let colors_lb = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .show_separators(true)
        .build();
    let colors_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .min_content_height(160)
        .max_content_height(280)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&colors_lb)
        .build();

    let refresh_colors_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh_colors: Rc<dyn Fn()> = Rc::new({
        let slot = refresh_colors_slot.clone();
        let colors_lb = colors_lb.clone();
        let st = state.clone();
        let dd = dd.clone();
        let refresh_main = refresh_main.clone();
        let w = window.clone();
        move || {
            let i = dd.selected() as usize;
            while let Some(c) = colors_lb.first_child() {
                colors_lb.remove(&c);
            }
            let colors: Vec<[u8; 4]> = {
                let g = st.borrow();
                g.palette_book
                    .entries
                    .get(i)
                    .map(|e| e.colors.clone())
                    .unwrap_or_default()
            };
            let can_remove = colors.len() > 1;
            for (ci, &col) in colors.iter().enumerate() {
                let row = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(8)
                    .margin_top(6)
                    .margin_bottom(6)
                    .margin_start(4)
                    .margin_end(4)
                    .build();
                let sw = gtk::DrawingArea::builder()
                    .width_request(28)
                    .height_request(22)
                    .build();
                let r = col[0] as f64 / 255.0;
                let gg = col[1] as f64 / 255.0;
                let b = col[2] as f64 / 255.0;
                let aa = col[3] as f64 / 255.0;
                sw.set_draw_func(move |_d, cr, w, h| {
                    cr.set_source_rgba(r, gg, b, aa);
                    cr.rectangle(0.0, 0.0, w as f64, h as f64);
                    let _ = cr.fill();
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
                    cr.set_line_width(1.0);
                    cr.rectangle(0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0);
                    let _ = cr.stroke();
                });
                let hex_s = if col[3] == 255 {
                    format!("#{:02x}{:02x}{:02x}", col[0], col[1], col[2])
                } else {
                    format!(
                        "#{:02x}{:02x}{:02x}{:02x}",
                        col[0], col[1], col[2], col[3]
                    )
                };
                let lbl = gtk::Label::new(Some(&hex_s));
                lbl.set_hexpand(true);
                lbl.set_xalign(0.0);
                let rm = gtk::Button::with_label("Remove");
                rm.set_sensitive(can_remove);
                rm.set_tooltip_text(Some("Remove this colour from the palette"));
                let st_rm = st.clone();
                let dd_rm = dd.clone();
                let w_rm = w.clone();
                let refresh_main_rm = refresh_main.clone();
                let slot_rm = slot.clone();
                rm.connect_clicked(move |_| {
                    let pi = dd_rm.selected() as usize;
                    let ok = {
                        let mut g = st_rm.borrow_mut();
                        g.palette_book.remove_color_at(pi, ci)
                    };
                    if !ok {
                        show_simple_alert(
                            &w_rm,
                            "Cannot remove",
                            "Each palette must keep at least one colour.",
                        );
                        return;
                    }
                    crate::settings::persist(&st_rm.borrow());
                    refresh_main_rm();
                    if let Some(r) = slot_rm.borrow().as_ref() {
                        r();
                    }
                });
                row.append(&sw);
                row.append(&lbl);
                row.append(&rm);
                colors_lb.append(&row);
            }
        }
    });
    *refresh_colors_slot.borrow_mut() = Some(refresh_colors.clone());

    let st_sel = state.clone();
    let ren_sel = rename_entry.clone();
    let refresh_colors_sel = refresh_colors.clone();
    dd.connect_selected_notify(move |dropdown| {
        let i = dropdown.selected() as usize;
        let g = st_sel.borrow();
        if let Some(e) = g.palette_book.entries.get(i) {
            ren_sel.set_text(&e.name);
        }
        drop(g);
        refresh_colors_sel();
    });
    let init_name = {
        let g = state.borrow();
        g.palette_book
            .entries
            .get(g.palette_book.active)
            .map(|e| e.name.clone())
    };
    if let Some(name) = init_name {
        rename_entry.set_text(&name);
    }

    let dup_btn = gtk::Button::with_label("Duplicate");
    let new_btn = gtk::Button::with_label("New empty");
    let del_btn = gtk::Button::with_label("Delete");
    let ren_btn = gtk::Button::with_label("Rename");

    let st_d = state.clone();
    let dd_d = dd.clone();
    let sync_d = sync_dd.clone();
    let refresh_d = refresh_main.clone();
    dup_btn.connect_clicked(move |_| {
        let i = dd_d.selected() as usize;
        let ok = {
            let mut g = st_d.borrow_mut();
            g.palette_book.duplicate_entry(i)
        };
        if ok {
            crate::settings::persist(&st_d.borrow());
            sync_d();
            refresh_d();
        }
    });

    let st_n = state.clone();
    let dd_n = dd.clone();
    let strings_n = strings.clone();
    let sync_n = sync_dd.clone();
    let refresh_n = refresh_main.clone();
    new_btn.connect_clicked(move |_| {
        {
            let mut g = st_n.borrow_mut();
            g.palette_book.new_empty_swatch();
        }
        crate::settings::persist(&st_n.borrow());
        sync_n();
        dd_n.set_selected(strings_n.n_items().saturating_sub(1));
        refresh_n();
    });

    let st_r = state.clone();
    let dd_r = dd.clone();
    let sync_r = sync_dd.clone();
    let refresh_r = refresh_main.clone();
    let ren_e = rename_entry.clone();
    let w_ren = window.clone();
    ren_btn.connect_clicked(move |_| {
        let i = dd_r.selected() as usize;
        let new_name = ren_e.text().to_string();
        let ok = {
            let mut g = st_r.borrow_mut();
            g.palette_book.rename(i, &new_name)
        };
        if !ok {
            show_simple_alert(&w_ren, "Rename failed", "Enter a non-empty name.");
            return;
        }
        crate::settings::persist(&st_r.borrow());
        sync_r();
        refresh_r();
    });

    let st_x = state.clone();
    let dd_x = dd.clone();
    let sync_x = sync_dd.clone();
    let refresh_x = refresh_main.clone();
    let w_x = window.clone();
    del_btn.connect_clicked(move |_| {
        let i = dd_x.selected() as usize;
        if i == 0 {
            show_simple_alert(
                &w_x,
                "Cannot delete",
                "The first palette is the built-in default and cannot be removed.",
            );
            return;
        }
        let removed = {
            let mut g = st_x.borrow_mut();
            g.palette_book.remove_at(i)
        };
        if !removed {
            show_simple_alert(&w_x, "Cannot delete", "Keep at least one palette.");
            return;
        }
        crate::settings::persist(&st_x.borrow());
        sync_x();
        refresh_x();
    });

    if state.borrow().palette_book.entries.len() <= 1 {
        del_btn.set_sensitive(false);
    }

    let row1 = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row1.append(&dup_btn);
    row1.append(&new_btn);
    row1.append(&del_btn);

    let row2 = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    row2.append(&rename_entry);
    row2.append(&ren_btn);

    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .spacing(12)
        .build();
    bx.append(&gtk::Label::new(Some("Palette")));
    bx.append(&dd);
    bx.append(&row1);
    let colors_hdr = gtk::Label::new(Some("Colours in palette"));
    colors_hdr.add_css_class("dim-label");
    colors_hdr.set_halign(gtk::Align::Start);
    bx.append(&colors_hdr);
    bx.append(&colors_scroll);
    bx.append(&gtk::Label::new(Some("Rename selected")));
    bx.append(&row2);

    let close = gtk::Button::with_label("Close");
    let d_close = d.clone();
    close.connect_clicked(move |_| d_close.close());

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    btn_row.append(&close);

    bx.append(&btn_row);
    d.set_content(Some(&bx));
    refresh_colors();

    let st_close = state.clone();
    let psc_close = ps.clone();
    d.connect_close_request(move |_| {
        st_close.borrow_mut().palette_book.clamp_active();
        crate::settings::persist(&st_close.borrow());
        refresh_palette_sidebar_full(&psc_close, &st_close);
        glib::Propagation::Proceed
    });

    d.present();
}

/// Overlapping squares: back square = secondary (right-click paint), front = primary (left-click paint).
/// Left-click anywhere swaps primary and secondary. Right-click a square chooses which slot the hue/SV picker edits.
fn make_fg_bg_selector(
    state: &SharedState,
    canvas: &CanvasCell,
    sv_da: &gtk::DrawingArea,
    picker_refresh: &PickerUiRefresh,
) -> gtk::DrawingArea {
    const BG_X: f64 = 2.0;
    const BG_Y: f64 = 2.0;
    const SQ: f64 = 26.0;
    const FG_X: f64 = 18.0;
    const FG_Y: f64 = 18.0;

    let da = gtk::DrawingArea::builder()
        .width_request(50)
        .height_request(42)
        .hexpand(false)
        .vexpand(false)
        .tooltip_text(
            "Back = secondary (right-click paint), front = primary (left-click). Left-click: swap colors. Right-click a square: load that slot into the picker. Left-drag sliders or the saturation/value square edits the front color; right-drag edits the back color.",
        )
        .build();
    let st_draw = state.clone();
    da.set_draw_func(move |_d, cr, _w, _h| {
        let st = st_draw.borrow();
        let fg = st.fg;
        let bg = st.bg;
        let pick = st.picker_target;
        let fill = |cr: &gtk::cairo::Context, c: [u8; 4], x: f64, y: f64, highlight: bool| {
            cr.set_source_rgba(
                c[0] as f64 / 255.0,
                c[1] as f64 / 255.0,
                c[2] as f64 / 255.0,
                c[3] as f64 / 255.0,
            );
            cr.rectangle(x, y, SQ, SQ);
            let _ = cr.fill();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            cr.set_line_width(1.0);
            cr.rectangle(x + 0.5, y + 0.5, SQ - 1.0, SQ - 1.0);
            let _ = cr.stroke();
            if highlight {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
                cr.set_line_width(2.0);
                cr.rectangle(x + 2.0, y + 2.0, SQ - 4.0, SQ - 4.0);
                let _ = cr.stroke();
            }
        };
        fill(cr, bg, BG_X, BG_Y, pick == ColorSlot::Right);
        fill(cr, fg, FG_X, FG_Y, pick == ColorSlot::Left);
    });

    let click = gtk::GestureClick::new();
    let st = state.clone();
    let cv = canvas.clone();
    let da_c = da.clone();
    let pr = picker_refresh.clone();
    let sv_c = sv_da.clone();
    click.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 {
            return;
        }
        let btn = gesture.current_button();
        let x = x as f64;
        let y = y as f64;
        let in_left = x >= FG_X && x < FG_X + SQ && y >= FG_Y && y < FG_Y + SQ;
        let in_right = x >= BG_X && x < BG_X + SQ && y >= BG_Y && y < BG_Y + SQ;
        let slot_at = if in_left {
            Some(ColorSlot::Left)
        } else if in_right {
            Some(ColorSlot::Right)
        } else {
            None
        };
        if btn == gdk::BUTTON_PRIMARY {
            let mut g = st.borrow_mut();
            let t = g.fg;
            g.fg = g.bg;
            g.bg = t;
            drop(g);
            picker_refresh_call(&pr);
            da_c.queue_draw();
            sv_c.queue_draw();
            queue_canvas(&cv);
            return;
        }
        if btn == gdk::BUTTON_SECONDARY {
            let Some(slot) = slot_at else {
                return;
            };
            {
                let mut g = st.borrow_mut();
                g.picker_target = slot;
                let c = match slot {
                    ColorSlot::Left => g.fg,
                    ColorSlot::Right => g.bg,
                };
                let (h, _, _) = rgb_bytes_to_hsv(c);
                g.picker_hue = h;
            }
            picker_refresh_call(&pr);
            da_c.queue_draw();
            sv_c.queue_draw();
        }
    });
    da.add_controller(click);
    da
}

fn refresh_recent_swatch_row(
    flow: &gtk::FlowBox,
    state: &SharedState,
    preview_da: &gtk::DrawingArea,
    cv: &CanvasCell,
    picker_refresh: &PickerUiRefresh,
) {
    while let Some(c) = flow.first_child() {
        flow.remove(&c);
    }
    let recents: Vec<[u8; 4]> = state.borrow().recent_colors.clone();
    for col in recents {
        let btn = swatch_button(col, false);
        let st = state.clone();
        let prev = preview_da.clone();
        let cv2 = cv.clone();
        let flow2 = flow.clone();
        let pr = picker_refresh.clone();
        let gc = gtk::GestureClick::new();
        gc.set_button(0);
        gc.connect_pressed(move |gesture, _, _, _| {
            {
                let mut g = st.borrow_mut();
                if gesture.current_button() == gdk::BUTTON_SECONDARY {
                    g.bg = col;
                } else {
                    g.fg = col;
                    push_recent_color(&mut g, col);
                }
            }
            picker_refresh_call(&pr);
            prev.queue_draw();
            queue_canvas(&cv2);
            refresh_recent_swatch_row(&flow2, &st, &prev, &cv2, &pr);
        });
        btn.add_controller(gc);
        flow.append(&btn);
    }
}

fn zoom_to_fit(state: &SharedState, canvas_cell: &CanvasCell) {
    let Some(ref da) = *canvas_cell.borrow() else { return };
    let vw = da.width() as f64;
    let vh = da.height() as f64;
    if vw <= 0.0 || vh <= 0.0 { return; }
    let mut st = state.borrow_mut();
    let dw = st.doc.width as f64;
    let dh = st.doc.height as f64;
    if dw <= 0.0 || dh <= 0.0 { return; }
    let pad = 16.0;
    let z = ((vw - 2.0 * pad) / dw).min((vh - 2.0 * pad) / dh).clamp(0.05, 32.0);
    st.zoom = z;
    st.pan_x = (vw - dw * z) / 2.0;
    st.pan_y = (vh - dh * z) / 2.0;
}

fn zoom_step(state: &SharedState, canvas_cell: &CanvasCell, factor: f64) {
    let Some(ref da) = *canvas_cell.borrow() else { return };
    let vw = da.width() as f64;
    let vh = da.height() as f64;
    let cx = vw / 2.0;
    let cy = vh / 2.0;
    let mut st = state.borrow_mut();
    let old_z = st.zoom;
    st.zoom = (st.zoom * factor).clamp(0.05, 32.0);
    let doc_x = (cx - st.pan_x) / old_z;
    let doc_y = (cy - st.pan_y) / old_z;
    st.pan_x = cx - doc_x * st.zoom;
    st.pan_y = cy - doc_y * st.zoom;
}

fn listbox_row_index(lb: &gtk::ListBox, target: &gtk::ListBoxRow) -> usize {
    let mut i = 0usize;
    let mut child = lb.first_child();
    while let Some(c) = child {
        if let Some(r) = c.downcast_ref::<gtk::ListBoxRow>() {
            if r == target {
                return i;
            }
            i += 1;
        }
        child = c.next_sibling();
    }
    0
}

fn refresh_layers_list(state: &SharedState, layers_cell: &LayersCell, canvas: &CanvasCell) {
    let Some(lb) = layers_cell.borrow().clone() else {
        return;
    };
    while let Some(c) = lb.first_child() {
        lb.remove(&c);
    }

    let layers_info: Vec<_> = {
        let doc = state.borrow();
        doc.doc
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (i, l.name.clone(), l.visible, l.opacity))
            .collect()
    };
    let active_layer = state.borrow().doc.active_layer;

    for (i, name, visible, opacity) in layers_info {
        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(6)
            .margin_end(6)
            .build();

        let top_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        let name_label = gtk::Label::builder()
            .label(&name)
            .hexpand(true)
            .xalign(0.0)
            .build();
        name_label.add_css_class("heading");
        let vis = gtk::Switch::builder()
            .active(visible)
            .valign(gtk::Align::Center)
            .build();
        top_row.append(&name_label);
        top_row.append(&vis);

        let bot_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        let op_adj = gtk::Adjustment::new(f64::from(opacity) * 100.0, 0.0, 100.0, 1.0, 5.0, 0.0);
        let op_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&op_adj));
        op_scale.set_draw_value(true);
        op_scale.set_value_pos(gtk::PositionType::Right);
        op_scale.set_digits(0);
        op_scale.set_hexpand(true);
        let up = gtk::Button::from_icon_name("go-up-symbolic");
        let down = gtk::Button::from_icon_name("go-down-symbolic");
        let merge = gtk::Button::from_icon_name("object-flip-vertical-symbolic");
        merge.set_tooltip_text(Some("Merge down"));
        let del = gtk::Button::from_icon_name("edit-delete-symbolic");
        del.set_tooltip_text(Some("Delete layer"));
        bot_row.append(&op_scale);
        bot_row.append(&up);
        bot_row.append(&down);
        bot_row.append(&merge);
        bot_row.append(&del);

        outer.append(&top_row);
        outer.append(&bot_row);

        let st = state.clone();
        let cv = canvas.clone();
        vis.connect_state_set(move |_sw, active| {
            let mut g = st.borrow_mut();
            if let Some(l) = g.doc.layers.get_mut(i) {
                l.visible = active;
                g.modified = true;
                g.bump_document_revision();
            }
            queue_canvas(&cv);
            glib::Propagation::Proceed
        });

        let st = state.clone();
        let cv = canvas.clone();
        op_adj.connect_value_changed(move |a| {
            let mut g = st.borrow_mut();
            if let Some(l) = g.doc.layers.get_mut(i) {
                l.opacity = (a.value() / 100.0) as f32;
                g.modified = true;
                g.bump_document_revision();
            }
            queue_canvas(&cv);
        });

        let st = state.clone();
        let lc2 = layers_cell.clone();
        let cv2 = canvas.clone();
        let idx = i;
        up.connect_clicked(move |_| {
            if idx > 0 {
                let mut g = st.borrow_mut();
                g.doc.move_layer(idx, idx - 1);
                g.bump_document_revision();
                drop(g);
                refresh_layers_list(&st, &lc2, &cv2);
                queue_canvas(&cv2);
            }
        });

        let st = state.clone();
        let lc3 = layers_cell.clone();
        let cv3 = canvas.clone();
        let idx_d = i;
        down.connect_clicked(move |_| {
            let mut g = st.borrow_mut();
            if idx_d + 1 < g.doc.layers.len() {
                g.doc.move_layer(idx_d, idx_d + 1);
                g.bump_document_revision();
                drop(g);
                refresh_layers_list(&st, &lc3, &cv3);
                queue_canvas(&cv3);
            }
        });

        let st = state.clone();
        let lc4 = layers_cell.clone();
        let cv4 = canvas.clone();
        let idx_merge = i;
        merge.connect_clicked(move |_| {
            let mut g = st.borrow_mut();
            if g.doc.merge_down(idx_merge) {
                g.history.clear();
                g.modified = true;
                g.bump_document_revision();
                drop(g);
                refresh_layers_list(&st, &lc4, &cv4);
                queue_canvas(&cv4);
            }
        });

        let st = state.clone();
        let lc5 = layers_cell.clone();
        let cv5 = canvas.clone();
        let idx_del = i;
        del.connect_clicked(move |btn| {
            if st.borrow().doc.layers.len() <= 1 {
                return;
            }
            let win = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            let st2 = st.clone();
            let lc = lc5.clone();
            let cv = cv5.clone();
            let d = libadwaita::Window::builder()
                .modal(true)
                .title("Delete layer")
                .default_width(340)
                .default_height(120)
                .build();
            if let Some(ref w) = win {
                d.set_transient_for(Some(w));
            }
            let label = gtk::Label::new(Some("Delete this layer?"));
            label.set_wrap(true);
            let cancel = gtk::Button::with_label("Cancel");
            let confirm = gtk::Button::with_label("Delete");
            confirm.add_css_class("destructive-action");
            let btn_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::End)
                .build();
            btn_row.append(&cancel);
            btn_row.append(&confirm);
            let bx = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_top(16)
                .margin_bottom(16)
                .margin_start(16)
                .margin_end(16)
                .spacing(16)
                .build();
            bx.append(&label);
            bx.append(&btn_row);
            d.set_content(Some(&bx));
            let dc = d.clone();
            cancel.connect_clicked(move |_| dc.close());
            let dc2 = d.clone();
            confirm.connect_clicked(move |_| {
                let mut g = st2.borrow_mut();
                if g.doc.remove_layer(idx_del) {
                    g.history.clear();
                    g.modified = true;
                    g.bump_document_revision();
                    drop(g);
                    refresh_layers_list(&st2, &lc, &cv);
                    queue_canvas(&cv);
                }
                dc2.close();
            });
            d.present();
        });

        let list_row = gtk::ListBoxRow::new();
        list_row.set_overflow(gtk::Overflow::Hidden);
        list_row.set_child(Some(&outer));
        lb.append(&list_row);
    }

    if let Some(row) = lb.row_at_index(active_layer as i32) {
        lb.select_row(Some(&row));
    }
}

thread_local! {
    static TRANSPARENCY_CHECKER: RefCell<Option<gtk::cairo::SurfacePattern>> = const { RefCell::new(None) };
}

/// 16×16 tile (8px checker cells), repeated — avoids O((W/8)²) cairo rectangles per frame.
fn straight_fg_to_cairo(fg: [u8; 4]) -> (f64, f64, f64, f64) {
    (
        fg[0] as f64 / 255.0,
        fg[1] as f64 / 255.0,
        fg[2] as f64 / 255.0,
        fg[3] as f64 / 255.0,
    )
}

/// Document-space overlay while dragging line / rect / ellipse (matches final geometry; Cairo stroke width ≈ brush diameter).
fn draw_shape_drag_preview(
    cr: &gtk::cairo::Context,
    tool: ToolKind,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    fg: [u8; 4],
    shape_filled: bool,
    brush_size: f64,
    brush_hardness: f64,
) {
    let (r, g, b, a) = straight_fg_to_cairo(fg);
    let lw = brush_size.max(0.5);
    cr.set_line_width(lw);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    match tool {
        ToolKind::Line => {
            cr.set_source_rgba(r, g, b, a);
            cr.move_to(x0, y0);
            cr.line_to(x1, y1);
            cr.stroke().unwrap();
        }
        ToolKind::Rect => {
            let min_x = x0.min(x1);
            let max_x = x0.max(x1);
            let min_y = y0.min(y1);
            let max_y = y0.max(y1);
            let rw = max_x - min_x;
            let rh = max_y - min_y;
            cr.rectangle(min_x, min_y, rw, rh);
            if shape_filled && rw > 0.0 && rh > 0.0 {
                cr.set_source_rgba(r, g, b, a);
                cr.fill_preserve().unwrap();
                cr.set_source_rgba(r, g, b, a);
                cr.stroke().unwrap();
            } else {
                cr.set_source_rgba(r, g, b, a);
                cr.stroke().unwrap();
            }
        }
        ToolKind::Ellipse => {
            let cx = (x0 + x1) * 0.5;
            let cy = (y0 + y1) * 0.5;
            let rx = (x1 - x0).abs() * 0.5;
            let ry = (y1 - y0).abs() * 0.5;
            if rx < 0.25 || ry < 0.25 {
                return;
            }
            let steps = ellipse_outline_segment_count(rx, ry, brush_size * 0.5, brush_hardness);
            cr.move_to(cx + rx, cy);
            for i in 1..=steps {
                let t = std::f64::consts::TAU * i as f64 / steps as f64;
                cr.line_to(cx + rx * t.cos(), cy + ry * t.sin());
            }
            cr.close_path();
            if shape_filled {
                cr.set_source_rgba(r, g, b, a);
                cr.fill_preserve().unwrap();
                cr.set_source_rgba(r, g, b, a);
                cr.stroke().unwrap();
            } else {
                cr.set_source_rgba(r, g, b, a);
                cr.stroke().unwrap();
            }
        }
        _ => {}
    }
}

fn with_transparency_checker_pattern(f: impl FnOnce(&gtk::cairo::SurfacePattern)) {
    TRANSPARENCY_CHECKER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 16, 16).unwrap();
            let crc = gtk::cairo::Context::new(&surf).unwrap();
            crc.set_source_rgb(0.93, 0.93, 0.93);
            crc.paint().unwrap();
            crc.set_source_rgb(0.78, 0.78, 0.78);
            for ry in 0..2i32 {
                for rx in 0..2i32 {
                    if (rx + ry) % 2 == 1 {
                        crc.rectangle(rx as f64 * 8.0, ry as f64 * 8.0, 8.0, 8.0);
                    }
                }
            }
            crc.fill().unwrap();
            let p = gtk::cairo::SurfacePattern::create(surf);
            p.set_extend(gtk::cairo::Extend::Repeat);
            p.set_filter(gtk::cairo::Filter::Nearest);
            *slot = Some(p);
        }
        f(slot.as_ref().unwrap());
    });
}

/// Build the magic-wand / region marquee outline using merged axis-aligned segments (same geometry
/// as per-pixel strokes, far fewer Cairo primitives on large selections).
fn region_mask_outline_path(cr: &gtk::cairo::Context, mask: &[u8], rw: u32, rh: u32) {
    let ww = rw as usize;
    let h = rh as usize;
    debug_assert_eq!(mask.len(), ww * h);

    for y in 0..h {
        let mut x = 0usize;
        while x < ww {
            let idx = y * ww + x;
            if mask[idx] == 0 {
                x += 1;
                continue;
            }
            let top_clear = y == 0 || mask[idx - ww] == 0;
            if !top_clear {
                x += 1;
                continue;
            }
            let x0 = x;
            x += 1;
            while x < ww {
                let i2 = y * ww + x;
                if mask[i2] == 0 {
                    break;
                }
                if !(y == 0 || mask[i2 - ww] == 0) {
                    break;
                }
                x += 1;
            }
            cr.move_to(x0 as f64, y as f64);
            cr.line_to(x as f64, y as f64);
        }
    }

    for y in 0..h {
        let mut x = 0usize;
        while x < ww {
            let idx = y * ww + x;
            if mask[idx] == 0 {
                x += 1;
                continue;
            }
            let bottom_clear = y + 1 == h || mask[idx + ww] == 0;
            if !bottom_clear {
                x += 1;
                continue;
            }
            let x0 = x;
            x += 1;
            let y1 = (y + 1) as f64;
            while x < ww {
                let i2 = y * ww + x;
                if mask[i2] == 0 {
                    break;
                }
                if !(y + 1 == h || mask[i2 + ww] == 0) {
                    break;
                }
                x += 1;
            }
            cr.move_to(x0 as f64, y1);
            cr.line_to(x as f64, y1);
        }
    }

    for x in 0..ww {
        let mut y = 0usize;
        while y < h {
            let idx = y * ww + x;
            if mask[idx] == 0 {
                y += 1;
                continue;
            }
            let left_clear = x == 0 || mask[idx - 1] == 0;
            if !left_clear {
                y += 1;
                continue;
            }
            let y0 = y;
            y += 1;
            let xf = x as f64;
            while y < h {
                let i2 = y * ww + x;
                if mask[i2] == 0 {
                    break;
                }
                if !(x == 0 || mask[i2 - 1] == 0) {
                    break;
                }
                y += 1;
            }
            cr.move_to(xf, y0 as f64);
            cr.line_to(xf, y as f64);
        }
    }

    for x in 0..ww {
        let mut y = 0usize;
        while y < h {
            let idx = y * ww + x;
            if mask[idx] == 0 {
                y += 1;
                continue;
            }
            let right_clear = x + 1 == ww || mask[idx + 1] == 0;
            if !right_clear {
                y += 1;
                continue;
            }
            let y0 = y;
            y += 1;
            let xf = (x + 1) as f64;
            while y < h {
                let i2 = y * ww + x;
                if mask[i2] == 0 {
                    break;
                }
                if !(x + 1 == ww || mask[i2 + 1] == 0) {
                    break;
                }
                y += 1;
            }
            cr.move_to(xf, y0 as f64);
            cr.line_to(xf, y as f64);
        }
    }
}

fn draw_canvas(state: &SharedState, cr: &gtk::cairo::Context) {
    let (
        w,
        h,
        pan_x,
        pan_y,
        zoom,
        show_pixel_grid,
        shape_preview,
        shape_preview_color,
        shape_filled,
        brush_size,
        brush_hardness,
    ) = {
        let st = state.borrow();
        let preview_c = if st.shape_drag_preview.is_some() {
            st.active_paint_color()
        } else {
            st.fg
        };
        (
            st.doc.width,
            st.doc.height,
            st.pan_x,
            st.pan_y,
            st.zoom,
            st.show_pixel_grid,
            st.shape_drag_preview,
            preview_c,
            st.shape_filled,
            st.brush_size,
            st.brush_hardness,
        )
    };
    let len = (w * h * 4) as usize;
    let w_i = w as i32;
    let h_i = h as i32;

    let composite_surface = {
        let mut st = state.borrow_mut();
        let use_cache = !st.brush_stroke_in_progress
            && st.composite_cache_at_revision == st.document_visual_revision
            && st.composite_cache_surface.as_ref().is_some_and(|s| {
                s.width() == w_i && s.height() == h_i
            });
        if !use_cache {
            let stroke_active = st.stroke_composite_active_layer;
            let below_temp = if st.brush_stroke_in_progress && st.stroke_composite_below_valid() {
                st.stroke_composite_below.take()
            } else {
                None
            };
            st.composite_cache_surface = None;
            {
                let AppState {
                    ref doc,
                    ref mut composite_cache_premul,
                    ..
                } = *st;
                composite_cache_premul.resize(len, 0);
                if let Some(ref below) = below_temp {
                    composite_layers_from_below_into(
                        composite_cache_premul,
                        doc.width,
                        doc.height,
                        &doc.layers,
                        stroke_active,
                        below,
                    );
                } else {
                    composite_layers_into(
                        composite_cache_premul,
                        doc.width,
                        doc.height,
                        &doc.layers,
                    );
                }
            }
            st.stroke_composite_below = below_temp;
            {
                let AppState {
                    ref composite_cache_premul,
                    ref mut composite_cache_surface,
                    ref mut composite_cache_at_revision,
                    document_visual_revision,
                    ..
                } = *st;
                let cairo_stride = gtk::cairo::Format::ARgb32
                    .stride_for_width(w)
                    .expect("cairo stride");
                let need_new = composite_cache_surface.as_ref().map_or(true, |s| {
                    s.width() != w_i || s.height() != h_i || s.stride() != cairo_stride
                });
                if need_new {
                    *composite_cache_surface = Some(
                        gtk::cairo::ImageSurface::create(
                            gtk::cairo::Format::ARgb32,
                            w_i,
                            h_i,
                        )
                        .expect("composite ImageSurface"),
                    );
                }
                let surf = composite_cache_surface.as_mut().expect("surface");
                {
                    let mut data = surf.data().expect("composite surface data");
                    premul_rgba_to_cairo_argb32(
                        &mut data,
                        cairo_stride as usize,
                        w,
                        h,
                        composite_cache_premul,
                    );
                }
                *composite_cache_at_revision = document_visual_revision;
            }
        }
        st.composite_cache_surface
            .as_ref()
            .expect("composite surface after rebuild")
            .clone()
    };

    cr.save().unwrap();
    cr.translate(pan_x, pan_y);
    cr.scale(zoom, zoom);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    cr.clip();
    with_transparency_checker_pattern(|pat| {
        cr.save().unwrap();
        cr.set_antialias(gtk::cairo::Antialias::None);
        cr.set_source(pat).unwrap();
        cr.paint().unwrap();
        cr.restore().unwrap();
    });
    cr.set_source_surface(composite_surface.as_ref(), 0.0, 0.0)
        .expect("set_source_surface composite");
    cr.source().set_filter(gtk::cairo::Filter::Nearest);
    cr.paint().unwrap();
    cr.restore().unwrap();

    if show_pixel_grid {
        cr.save().unwrap();
        cr.translate(pan_x, pan_y);
        cr.scale(zoom, zoom);
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        cr.clip();
        let lw = 1.0 / zoom.max(0.001);
        cr.set_line_width(lw);
        cr.set_antialias(gtk::cairo::Antialias::None);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
        for x in 0..=w {
            let xf = x as f64;
            cr.move_to(xf, 0.0);
            cr.line_to(xf, h as f64);
        }
        for y in 0..=h {
            let yf = y as f64;
            cr.move_to(0.0, yf);
            cr.line_to(w as f64, yf);
        }
        cr.stroke().unwrap();
        cr.restore().unwrap();
    }

    let floating_pixbuf = {
        let mut st = state.borrow_mut();
        if st.floating.is_none() {
            st.floating_pixbuf_cache = None;
            st.floating_pixbuf_key = None;
            None
        } else {
            let key = {
                let f = st.floating.as_ref().unwrap();
                let fw = f.w.max(1);
                let fh = f.h.max(1);
                (f.data.as_ptr() as usize, f.data.len(), fw, fh)
            };
            let hit = st.floating_pixbuf_key == Some(key) && st.floating_pixbuf_cache.is_some();
            if hit {
                st.floating_pixbuf_cache.clone()
            } else {
                let f = st.floating.take().unwrap();
                let fw = f.w.max(1);
                let fh = f.h.max(1);
                let fl_len = (fw * fh * 4) as usize;
                st.floating_straight_scratch.resize(fl_len, 0);
                premul_to_straight_rgba_into(&mut st.floating_straight_scratch, &f.data);
                let bytes = glib::Bytes::from_owned(std::mem::replace(
                    &mut st.floating_straight_scratch,
                    Vec::new(),
                ));
                let pb = Pixbuf::from_bytes(
                    &bytes,
                    gdk_pixbuf::Colorspace::Rgb,
                    true,
                    8,
                    fw,
                    fh,
                    fw * 4,
                );
                st.floating_pixbuf_cache = Some(pb.clone());
                st.floating_pixbuf_key = Some(key);
                st.floating = Some(f);
                Some(pb)
            }
        }
    };

    if let Some(pb) = floating_pixbuf {
        let st_fl = state.borrow();
        if let Some(f) = st_fl.floating.as_ref() {
            cr.save().unwrap();
            cr.translate(pan_x, pan_y);
            cr.scale(zoom, zoom);
            let m = floating_image_to_doc_matrix(f);
            cr.transform(m);
            cr.set_source_pixbuf(&pb, 0.0, 0.0);
            let filt = if (f.scale_x - 1.0).abs() < 1e-6
                && (f.scale_y - 1.0).abs() < 1e-6
                && f.angle_deg.rem_euclid(90.0).abs() < 1e-6
            {
                gtk::cairo::Filter::Nearest
            } else {
                gtk::cairo::Filter::Good
            };
            cr.source().set_filter(filt);
            cr.paint().unwrap();
            cr.restore().unwrap();

            let quad = floating_quad_doc(f);
            cr.save().unwrap();
            cr.translate(pan_x, pan_y);
            cr.scale(zoom, zoom);
            cr.set_dash(&[6.0, 6.0], 0.0);
            cr.set_line_width(1.0 / zoom.max(0.001));
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
            cr.move_to(quad[0].0, quad[0].1);
            cr.line_to(quad[1].0, quad[1].1);
            cr.line_to(quad[2].0, quad[2].1);
            cr.line_to(quad[3].0, quad[3].1);
            cr.close_path();
            cr.stroke().unwrap();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.95);
            cr.set_dash(&[6.0, 6.0], 6.0);
            cr.move_to(quad[0].0, quad[0].1);
            cr.line_to(quad[1].0, quad[1].1);
            cr.line_to(quad[2].0, quad[2].1);
            cr.line_to(quad[3].0, quad[3].1);
            cr.close_path();
            cr.stroke().unwrap();
            draw_floating_handles(cr, zoom, f);
            cr.restore().unwrap();
        }
    }

    let st_draw = state.borrow();
    let selection = st_draw.selection.as_ref();

    if let Some(sel) = selection {
        cr.save().unwrap();
        cr.translate(pan_x, pan_y);
        cr.scale(zoom, zoom);
        let lw = 1.0 / zoom.max(0.001);
        cr.set_line_width(lw);
        match sel {
            Selection::Rect(sx, sy, sw, sh) => {
                cr.set_dash(&[6.0, 6.0], 0.0);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                cr.rectangle(*sx as f64, *sy as f64, *sw as f64, *sh as f64);
                cr.stroke().unwrap();
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.95);
                cr.set_dash(&[6.0, 6.0], 6.0);
                cr.rectangle(*sx as f64, *sy as f64, *sw as f64, *sh as f64);
                cr.stroke().unwrap();
            }
            Selection::Region {
                width: rw,
                height: rh,
                mask,
                ..
            } => {
                if *rw == w && *rh == h && mask.len() == (w * h) as usize {
                    cr.new_path();
                    region_mask_outline_path(cr, mask, *rw, *rh);
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                    cr.set_dash(&[6.0, 6.0], 0.0);
                    cr.stroke_preserve().unwrap();
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.95);
                    cr.set_dash(&[6.0, 6.0], 6.0);
                    cr.stroke().unwrap();
                }
            }
        }
        cr.restore().unwrap();
    }

    if let Some((tool, x0, y0, x1, y1)) = shape_preview {
        cr.save().unwrap();
        cr.translate(pan_x, pan_y);
        cr.scale(zoom, zoom);
        draw_shape_drag_preview(
            cr,
            tool,
            x0,
            y0,
            x1,
            y1,
            shape_preview_color,
            shape_filled,
            brush_size,
            brush_hardness,
        );
        cr.restore().unwrap();
    }
}

fn commit_floating(state: &mut AppState) {
    let Some(f) = state.floating.take() else {
        return;
    };
    state.floating_drag = None;
    state.move_grab_doc = None;
    let idx = state.doc.active_layer;
    let Some(layer) = state.doc.layers.get_mut(idx) else {
        return;
    };
    let before = layer.pixels.clone();
    let trivial = f.angle_deg.rem_euclid(360.0).abs() < 1e-6
        && (f.scale_x - 1.0).abs() < 1e-6
        && (f.scale_y - 1.0).abs() < 1e-6
        && !f.flip_h
        && !f.flip_v;
    if trivial {
        paste_rect(
            layer,
            f.x.round() as i32,
            f.y.round() as i32,
            f.w,
            f.h,
            &f.data,
        );
    } else if let Some((px, py, rw, rh, buf)) = rasterize_floating_to_premul(&f) {
        paste_rect(layer, px, py, rw, rh, &buf);
    } else {
        paste_rect(
            layer,
            f.x.round() as i32,
            f.y.round() as i32,
            f.w,
            f.h,
            &f.data,
        );
    }
    if layer.pixels != before {
        state.history.commit_change(idx, before);
        state.modified = true;
        state.bump_document_revision();
    }
}

fn paint_brush_drag_sample(
    st: &mut AppState,
    wx: f64,
    wy: f64,
    last_sample: &RefCell<Option<(f64, f64)>>,
) -> bool {
    const EPS: f64 = 1e-4;
    if last_sample.borrow().is_some_and(|(px, py)| {
        (wx - px).abs() < EPS && (wy - py).abs() < EPS
    }) {
        return false;
    }
    let (cx, cy) = st.widget_to_doc(wx, wy);
    let Some((lx, ly)) = st.last_doc_pos else {
        st.last_doc_pos = Some((cx, cy));
        *last_sample.borrow_mut() = Some((wx, wy));
        return false;
    };
    let eraser = st.tool == ToolKind::Eraser;
    let paint_color = if eraser {
        st.fg
    } else {
        st.active_paint_color()
    };
    let layer = match st.doc.active_layer_mut() {
        Some(l) => l,
        None => return false,
    };
    match st.tool {
        ToolKind::Brush | ToolKind::Eraser => {
            let radius = st.brush_size * 0.5;
            stroke_line(
                layer,
                lx,
                ly,
                cx,
                cy,
                radius,
                st.brush_hardness,
                paint_color,
                eraser,
            );
        }
        ToolKind::Pixel => {
            stroke_line_square(
                layer,
                lx,
                ly,
                cx,
                cy,
                st.brush_size,
                paint_color,
                false,
            );
        }
        _ => return false,
    }
    st.last_doc_pos = Some((cx, cy));
    *last_sample.borrow_mut() = Some((wx, wy));
    st.modified = true;
    true
}

fn remove_brush_paint_tick(slot: &RefCell<Option<gtk::TickCallbackId>>) {
    if let Some(id) = slot.borrow_mut().take() {
        id.remove();
    }
}

/// While a brush stroke is active, GTK may deliver very few `GestureDrag::update` events (notably on
/// Wayland). Sampling each frame keeps strokes dense. Coordinates **must** match `drag_update`
/// (`press + offset`); using raw `device_position` + surface math disagrees with widget space and
/// produces broken, “laddered” strokes.
fn install_brush_paint_tick(
    canvas: &gtk::DrawingArea,
    gesture: &gtk::GestureDrag,
    state: &SharedState,
    canvas_cell: &CanvasCell,
    last_brush_widget: &Rc<RefCell<Option<(f64, f64)>>>,
    brush_widget_start: &Rc<RefCell<Option<(f64, f64)>>>,
    tick_slot: &Rc<RefCell<Option<gtk::TickCallbackId>>>,
) {
    remove_brush_paint_tick(tick_slot.as_ref());
    let st_c = state.clone();
    let cv_c = canvas_cell.clone();
    let lbs_c = last_brush_widget.clone();
    let bws_c = brush_widget_start.clone();
    let gesture_c = gesture.clone();
    *tick_slot.borrow_mut() = Some(canvas.add_tick_callback(move |_w, _fc| {
        {
            let st = st_c.borrow();
            if !st.brush_stroke_in_progress {
                return ControlFlow::Continue;
            }
            if !matches!(
                st.tool,
                ToolKind::Brush | ToolKind::Eraser | ToolKind::Pixel
            ) {
                return ControlFlow::Continue;
            }
        }
        let Some((ox, oy)) = gesture_c.offset() else {
            return ControlFlow::Continue;
        };
        let Some((bx, by)) = *bws_c.borrow() else {
            return ControlFlow::Continue;
        };
        let wx = bx + ox;
        let wy = by + oy;
        let mut st = st_c.borrow_mut();
        let painted = paint_brush_drag_sample(&mut st, wx, wy, &lbs_c);
        drop(st);
        if painted {
            queue_canvas(&cv_c);
        }
        ControlFlow::Continue
    }));
}

fn setup_canvas_input(
    canvas: &gtk::DrawingArea,
    state: &SharedState,
    canvas_cell: &CanvasCell,
    color_preview_da_cell: &ColorPreviewDaCell,
    recent_swatches: &gtk::FlowBox,
    picker_ui_refresh: &PickerUiRefresh,
) {
    let brush_widget_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));
    let last_brush_widget: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));
    let brush_paint_tick: Rc<RefCell<Option<gtk::TickCallbackId>>> = Rc::new(RefCell::new(None));
    let move_widget_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));

    let drag = gtk::GestureDrag::new();
    drag.set_button(0);
    let drag_brush_tick = drag.clone();
    let st_drag_begin = state.clone();
    let cv_drag = canvas_cell.clone();
    let cnv = canvas.clone();
    let bpt_begin = brush_paint_tick.clone();
    let bws = brush_widget_start.clone();
    let lbs_begin = last_brush_widget.clone();
    let mws_b = move_widget_start.clone();
    let cb_drag = color_preview_da_cell.clone();
    let recent_drag = recent_swatches.clone();
    let picker_drag = picker_ui_refresh.clone();
    drag.connect_drag_begin(move |gesture, wx, wy| {
        cnv.grab_focus();
        let mut st = st_drag_begin.borrow_mut();
        let btn = gesture.current_button();
        st.pointer_drag_button = if btn == 0 {
            gdk::BUTTON_PRIMARY
        } else {
            btn
        };
        if st.floating.is_some() && st.tool != ToolKind::Move {
            commit_floating(&mut st);
        }
        let (dx, dy) = st.widget_to_doc(wx, wy);
        let mut eyedrop_updated = false;
        let mut start_brush_paint_tick = false;
        match st.tool {
            ToolKind::Brush | ToolKind::Eraser => {
                if st.doc.active_layer_ref().is_none() {
                    return;
                }
                let eraser = st.tool == ToolKind::Eraser;
                let color = if eraser {
                    st.fg
                } else {
                    st.active_paint_color()
                };
                let radius = st.brush_size * 0.5;
                let hardness = st.brush_hardness;
                st.brush_stroke_in_progress = true;
                st.begin_stroke_undo();
                st.capture_stroke_composite_below();
                st.last_doc_pos = Some((dx, dy));
                *bws.borrow_mut() = Some((wx, wy));
                let layer = st.doc.active_layer_mut().expect("checked");
                stamp_circle(layer, dx, dy, radius, hardness, color, eraser);
                *lbs_begin.borrow_mut() = Some((wx, wy));
                st.modified = true;
                start_brush_paint_tick = true;
            }
            ToolKind::Pixel => {
                if st.doc.active_layer_ref().is_none() {
                    return;
                }
                let color = st.active_paint_color();
                let size = st.brush_size;
                st.brush_stroke_in_progress = true;
                st.begin_stroke_undo();
                st.capture_stroke_composite_below();
                st.last_doc_pos = Some((dx, dy));
                *bws.borrow_mut() = Some((wx, wy));
                let layer = st.doc.active_layer_mut().expect("checked");
                stamp_square(layer, dx, dy, size, color, false);
                *lbs_begin.borrow_mut() = Some((wx, wy));
                st.modified = true;
                start_brush_paint_tick = true;
            }
            ToolKind::Fill => {
                let fg = st.active_paint_color();
                let tol = st.fill_tolerance;
                let dw = st.doc.width;
                let dh = st.doc.height;
                st.begin_stroke_undo();
                if let Some(layer) = st.doc.active_layer_mut() {
                    let pv = straight_to_premul(&[fg[0], fg[1], fg[2], fg[3]]);
                    let fill = [pv[0], pv[1], pv[2], pv[3]];
                    flood_fill(
                        layer,
                        dx.floor().clamp(0.0, dw as f64 - 1.0) as u32,
                        dy.floor().clamp(0.0, dh as f64 - 1.0) as u32,
                        fill,
                        tol,
                    );
                    st.modified = true;
                }
                st.commit_stroke_undo();
            }
            ToolKind::Eyedropper => {
                let cw = st.doc.width;
                let ch = st.doc.height;
                let clen = (cw * ch * 4) as usize;
                let xi = dx.floor() as i32;
                let yi = dy.floor() as i32;
                let c = if st.brush_stroke_in_progress && st.stroke_composite_below_valid() {
                    let AppState {
                        ref doc,
                        ref mut composite_cache_premul,
                        ref mut stroke_composite_below,
                        stroke_composite_active_layer,
                        ..
                    } = *st;
                    let below = stroke_composite_below.take().expect("stroke below");
                    composite_cache_premul.resize(clen, 0);
                    composite_layers_from_below_into(
                        composite_cache_premul,
                        cw,
                        ch,
                        &doc.layers,
                        stroke_composite_active_layer,
                        &below,
                    );
                    *stroke_composite_below = Some(below);
                    sample_composite_premul(composite_cache_premul, cw, ch, xi, yi)
                } else if !st.brush_stroke_in_progress
                    && st.composite_cache_at_revision == st.document_visual_revision
                    && st.composite_cache_premul.len() == clen
                {
                    sample_composite_premul(st.composite_cache_premul.as_slice(), cw, ch, xi, yi)
                } else {
                    let AppState {
                        ref doc,
                        ref mut composite_cache_premul,
                        ..
                    } = *st;
                    composite_cache_premul.resize(clen, 0);
                    composite_layers_into(composite_cache_premul, cw, ch, &doc.layers);
                    sample_composite_premul(composite_cache_premul.as_slice(), cw, ch, xi, yi)
                };
                if st.pointer_drag_button == gdk::BUTTON_SECONDARY {
                    st.bg = c;
                } else {
                    st.fg = c;
                    push_recent_color(&mut st, c);
                }
                let (h, _, _) = rgb_bytes_to_hsv(c);
                st.picker_hue = h.rem_euclid(1.0);
                eyedrop_updated = true;
            }
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => {
                st.shape_drag_preview = None;
                st.drag_start_doc = Some((dx, dy));
            }
            ToolKind::SelectRect => {
                st.drag_start_doc = Some((dx, dy));
            }
            ToolKind::MagicSelect => {
                let dw = st.doc.width;
                let dh = st.doc.height;
                if let Some(layer) = st.doc.active_layer_ref() {
                    let x = dx.floor().clamp(0.0, dw as f64 - 1.0) as u32;
                    let y = dy.floor().clamp(0.0, dh as f64 - 1.0) as u32;
                    let (mask, wand_bbox) = flood_select_mask(layer, x, y, st.fill_tolerance);
                    if mask.iter().any(|&v| v != 0) {
                        st.selection = Some(Selection::Region {
                            width: dw,
                            height: dh,
                            mask,
                            tight_bbox: wand_bbox,
                        });
                    }
                }
            }
            ToolKind::Hand => {
                *bws.borrow_mut() = Some((st.pan_x, st.pan_y));
                tool_cursors::set_canvas_grabbing(&cnv);
            }
            ToolKind::Move => {
                st.floating_drag = None;
                st.move_grab_doc = None;
                if let Some(f) = st.floating.clone() {
                    match float_press_at(dx, dy, st.zoom, &f) {
                        FloatPress::Outside => {
                            commit_floating(&mut st);
                        }
                        FloatPress::Rotate => {
                            let (cx, cy) = floating_transform_center(&f);
                            let start_pointer_rad = (dy - cy).atan2(dx - cx);
                            st.floating_drag = Some(FloatingDrag::Rotate {
                                base_angle_deg: f.angle_deg,
                                start_pointer_rad,
                            });
                            *mws_b.borrow_mut() = Some((wx, wy));
                        }
                        FloatPress::Corner(i) => {
                            let m = floating_image_to_doc_matrix(&f);
                            let fw = f.w.max(1) as f64;
                            let fh = f.h.max(1) as f64;
                            let opp = (i + 2) % 4;
                            let (alx, aly) = corner_local(opp, fw, fh);
                            let anchor_doc = m.transform_point(alx, aly);
                            st.floating_drag = Some(FloatingDrag::ResizeCorner {
                                dragged_corner: i,
                                anchor_doc,
                            });
                            *mws_b.borrow_mut() = Some((wx, wy));
                        }
                        FloatPress::Edge(e) => {
                            let m = floating_image_to_doc_matrix(&f);
                            let fw = f.w.max(1) as f64;
                            let fh = f.h.max(1) as f64;
                            let (ax, ay) = edge_anchor_local_for_resize(e, fw, fh);
                            let anchor_doc = m.transform_point(ax, ay);
                            st.floating_drag = Some(FloatingDrag::ResizeEdge { edge: e, anchor_doc });
                            *mws_b.borrow_mut() = Some((wx, wy));
                        }
                        FloatPress::Body => {
                            st.floating_drag = Some(FloatingDrag::Move {
                                grab_off_x: dx - f.x,
                                grab_off_y: dy - f.y,
                            });
                            *mws_b.borrow_mut() = Some((wx, wy));
                        }
                    }
                }
                               if st.floating.is_none() {
                    if let Some(sel) = st.selection.clone() {
                        if sel.contains_point(dx, dy) {
                            let li = st.doc.active_layer;
                            match sel {
                                Selection::Rect(sx, sy, sw, sh) => {
                                    let layer = match st.doc.active_layer_mut() {
                                        Some(l) => l,
                                        None => return,
                                    };
                                    let before = layer.pixels.clone();
                                    let data = copy_rect(layer, sx, sy, sw, sh);
                                    clear_rect(layer, sx, sy, sw, sh);
                                    st.history.commit_change(li, before);
                                    st.bump_document_revision();
                                    st.floating = Some(FloatingSelection::new_pasted(
                                        sw,
                                        sh,
                                        data,
                                        sx as f64,
                                        sy as f64,
                                    ));
                                    st.selection = None;
                                    st.modified = true;
                                }
                                Selection::Region {
                                    width,
                                    height,
                                    mask,
                                    tight_bbox,
                                } => {
                                    if width != st.doc.width
                                        || height != st.doc.height
                                        || mask.len() != (width * height) as usize
                                    {
                                        return;
                                    }
                                    let Some((bx, by, bw, bh)) =
                                        region_tight_bbox_or_hint(&mask, width, height, tight_bbox)
                                    else {
                                        return;
                                    };
                                    let layer = match st.doc.active_layer_mut() {
                                        Some(l) => l,
                                        None => return,
                                    };
                                    let before = layer.pixels.clone();
                                    let data = copy_region_masked(layer, &mask, bx, by, bw, bh);
                                    clear_region_masked(layer, &mask);
                                    st.history.commit_change(li, before);
                                    st.bump_document_revision();
                                    st.floating = Some(FloatingSelection::new_pasted(
                                        bw,
                                        bh,
                                        data,
                                        bx as f64,
                                        by as f64,
                                    ));
                                    st.selection = None;
                                    st.modified = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        drop(st);
        if start_brush_paint_tick {
            install_brush_paint_tick(
                &cnv,
                &drag_brush_tick,
                &st_drag_begin,
                &cv_drag,
                &lbs_begin,
                &bws,
                &bpt_begin,
            );
        }
        if eyedrop_updated {
            if let Some(ref da) = *cb_drag.borrow() {
                picker_refresh_call(&picker_drag);
                da.queue_draw();
                refresh_recent_swatch_row(
                    &recent_drag,
                    &st_drag_begin,
                    da,
                    &cv_drag,
                    &picker_drag,
                );
            }
        }
        queue_canvas(&cv_drag);
        cnv.queue_draw();
    });

    let st_drag_up = state.clone();
    let drag_coalesce = Rc::new(RefCell::new(false));
    let cv_co = canvas_cell.clone();
    let dco = drag_coalesce.clone();
    let bws_up = brush_widget_start.clone();
    let lbs_up = last_brush_widget.clone();
    let mws_u = move_widget_start.clone();
    drag.connect_drag_update(move |_g, ox, oy| {
        let mut st = st_drag_up.borrow_mut();
        match st.tool {
            ToolKind::Brush | ToolKind::Eraser => {
                let Some((bx, by)) = *bws_up.borrow() else {
                    return;
                };
                let cur_wx = bx + ox;
                let cur_wy = by + oy;
                paint_brush_drag_sample(&mut st, cur_wx, cur_wy, &lbs_up);
            }
            ToolKind::Pixel => {
                let Some((bx, by)) = *bws_up.borrow() else {
                    return;
                };
                let cur_wx = bx + ox;
                let cur_wy = by + oy;
                paint_brush_drag_sample(&mut st, cur_wx, cur_wy, &lbs_up);
            }
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => {
                let Some((x0, y0)) = st.drag_start_doc else {
                    return;
                };
                let start_wx = x0 * st.zoom + st.pan_x;
                let start_wy = y0 * st.zoom + st.pan_y;
                let cur_wx = start_wx + ox;
                let cur_wy = start_wy + oy;
                let (cx, cy) = st.widget_to_doc(cur_wx, cur_wy);
                st.shape_drag_preview = Some((st.tool, x0, y0, cx, cy));
            }
            ToolKind::SelectRect => {
                let Some((x0, y0)) = st.drag_start_doc else {
                    return;
                };
                let start_wx = x0 * st.zoom + st.pan_x;
                let start_wy = y0 * st.zoom + st.pan_y;
                let cur_wx = start_wx + ox;
                let cur_wy = start_wy + oy;
                let (cx, cy) = st.widget_to_doc(cur_wx, cur_wy);
                let (rx, ry, rw, rh) = AppState::normalize_rect(x0, y0, cx, cy);
                st.selection = Some(Selection::Rect(rx, ry, rw, rh));
            }
            ToolKind::Hand => {
                if let Some((px0, py0)) = *bws_up.borrow() {
                    st.pan_x = px0 + ox;
                    st.pan_y = py0 + oy;
                }
            }
            ToolKind::Move => {
                let wpress = *mws_u.borrow();
                let px = st.pan_x;
                let py = st.pan_y;
                let z = st.zoom;
                let Some((wpx, wpy)) = wpress else {
                    return;
                };
                let (cx, cy) = ((wpx + ox - px) / z, (wpy + oy - py) / z);
                let drag = st.floating_drag;
                if let (Some(ref mut f), Some(d)) = (&mut st.floating, drag) {
                    match d {
                        FloatingDrag::Move {
                            grab_off_x,
                            grab_off_y,
                        } => {
                            f.x = cx - grab_off_x;
                            f.y = cy - grab_off_y;
                        }
                        FloatingDrag::Rotate {
                            base_angle_deg,
                            start_pointer_rad,
                        } => {
                            let (pcx, pcy) = floating_transform_center(f);
                            let ang = (cy - pcy).atan2(cx - pcx);
                            f.angle_deg = base_angle_deg
                                + (ang - start_pointer_rad) * 180.0 / std::f64::consts::PI;
                        }
                        FloatingDrag::ResizeCorner {
                            dragged_corner,
                            anchor_doc,
                        } => {
                            apply_floating_resize_corner(f, dragged_corner, anchor_doc, (cx, cy));
                        }
                        FloatingDrag::ResizeEdge { edge, anchor_doc } => {
                            apply_floating_resize_edge(f, edge, anchor_doc, (cx, cy));
                        }
                    }
                }
            }
            _ => {}
        }
        drop(st);
        if !*dco.borrow() {
            *dco.borrow_mut() = true;
            let cv = cv_co.clone();
            let flg = dco.clone();
            glib::idle_add_local_once(move || {
                *flg.borrow_mut() = false;
                queue_canvas(&cv);
                if let Some(ref c) = *cv.borrow() {
                    c.queue_draw();
                }
            });
        }
    });

    let st_drag_end = state.clone();
    let cv_drag_end = canvas_cell.clone();
    let cnv3 = canvas.clone();
    let bws_end = brush_widget_start.clone();
    let lbs_end = last_brush_widget.clone();
    let bpt_end = brush_paint_tick.clone();
    let mws_e = move_widget_start.clone();
    drag.connect_drag_end(move |_g, ox, oy| {
        let mut st = st_drag_end.borrow_mut();
        match st.tool {
            ToolKind::Brush | ToolKind::Eraser | ToolKind::Pixel => {
                remove_brush_paint_tick(bpt_end.as_ref());
                *bws_end.borrow_mut() = None;
                *lbs_end.borrow_mut() = None;
                st.commit_stroke_undo();
                st.clear_stroke_composite_below();
                st.brush_stroke_in_progress = false;
                st.last_doc_pos = None;
            }
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => {
                st.shape_drag_preview = None;
                if let Some((sx, sy)) = st.drag_start_doc {
                    let start_wx = sx * st.zoom + st.pan_x;
                    let start_wy = sy * st.zoom + st.pan_y;
                    let cur_wx = start_wx + ox;
                    let cur_wy = start_wy + oy;
                    let (cx, cy) = st.widget_to_doc(cur_wx, cur_wy);
                    let tool = st.tool;
                    let color = st.active_paint_color();
                    let filled = st.shape_filled;
                    let r = st.brush_size * 0.5;
                    let h = st.brush_hardness;
                    st.begin_stroke_undo();
                    let layer = match st.doc.active_layer_mut() {
                        Some(l) => l,
                        None => return,
                    };
                    match tool {
                        ToolKind::Line => {
                            stroke_line(layer, sx, sy, cx, cy, r, h, color, false);
                        }
                        ToolKind::Rect => {
                            draw_rect_outline(layer, sx, sy, cx, cy, r, h, color, filled, false);
                        }
                        ToolKind::Ellipse => {
                            draw_ellipse(layer, sx, sy, cx, cy, r, h, color, filled, false);
                        }
                        _ => {}
                    }
                    st.commit_stroke_undo();
                    st.modified = true;
                }
                st.drag_start_doc = None;
            }
            ToolKind::SelectRect => {
                if let Some((sx, sy)) = st.drag_start_doc {
                    let start_wx = sx * st.zoom + st.pan_x;
                    let start_wy = sy * st.zoom + st.pan_y;
                    let cur_wx = start_wx + ox;
                    let cur_wy = start_wy + oy;
                    let (cx, cy) = st.widget_to_doc(cur_wx, cur_wy);
                    let (rx, ry, rw, rh) = AppState::normalize_rect(sx, sy, cx, cy);
                    st.selection = Some(Selection::Rect(rx, ry, rw, rh));
                }
                st.drag_start_doc = None;
            }
            ToolKind::Hand => {
                *bws_end.borrow_mut() = None;
            }
            ToolKind::Move => {
                st.move_grab_doc = None;
                st.floating_drag = None;
                st.drag_start_doc = None;
                *mws_e.borrow_mut() = None;
            }
            _ => {}
        }
        drop(st);
        queue_canvas(&cv_drag_end);
        tool_cursors::sync_canvas_tool_cursor(&cnv3, st_drag_end.borrow().tool);
        cnv3.queue_draw();
    });

    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let st_m = state.clone();
    let cv_m = canvas_cell.clone();
    let cnv_m = canvas.clone();
    let middle_pan = Rc::new(std::cell::Cell::new(false));
    let last = Rc::new(RefCell::new(None::<(f64, f64)>));
    let last_c = last.clone();
    motion.connect_motion(move |ec, x, y| {
        let mask = ec.current_event_state();
        let m2 = mask.contains(gdk::ModifierType::BUTTON2_MASK);
        if middle_pan.get() && !m2 {
            middle_pan.set(false);
            tool_cursors::sync_canvas_tool_cursor(&cnv_m, st_m.borrow().tool);
        }
        if m2 {
            if !middle_pan.get() {
                middle_pan.set(true);
                tool_cursors::set_canvas_grabbing(&cnv_m);
            }
            let Some((lx, ly)) = *last_c.borrow() else {
                *last_c.borrow_mut() = Some((x, y));
                return;
            };
            let mut st = st_m.borrow_mut();
            st.pan_x += x - lx;
            st.pan_y += y - ly;
            *last_c.borrow_mut() = Some((x, y));
            drop(st);
            queue_canvas(&cv_m);
            return;
        }
        *last_c.borrow_mut() = Some((x, y));
    });
    canvas.add_controller(motion);
    canvas.add_controller(drag);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let st_s = state.clone();
    let cv_s = canvas_cell.clone();
    let last_s = last.clone();
    let cnv_s = canvas.clone();
    scroll.connect_scroll(move |_ec, _dx, dy| {
        let (x, y) = last_s
            .borrow()
            .unwrap_or((cnv_s.width() as f64 / 2.0, cnv_s.height() as f64 / 2.0));
        let mut st = st_s.borrow_mut();
        let factor = if dy > 0.0 { 1.0 / 1.1 } else { 1.1 };
        let old_z = st.zoom;
        st.zoom = (st.zoom * factor).clamp(0.05, 32.0);
        let doc_x = (x - st.pan_x) / old_z;
        let doc_y = (y - st.pan_y) / old_z;
        st.pan_x = x - doc_x * st.zoom;
        st.pan_y = y - doc_y * st.zoom;
        drop(st);
        queue_canvas(&cv_s);
        glib::Propagation::Stop
    });
    canvas.add_controller(scroll);
}

fn cut_selection(state: &SharedState) {
    let mut st = state.borrow_mut();
    let Some(sel) = st.selection.clone() else {
        return;
    };
    let idx = st.doc.active_layer;
    let Some(layer) = st.doc.layers.get_mut(idx) else {
        return;
    };
    let before = layer.pixels.clone();
    match sel {
        Selection::Rect(sx, sy, sw, sh) => {
            let data = copy_rect(layer, sx, sy, sw, sh);
            clear_rect(layer, sx, sy, sw, sh);
            st.clipboard = Some((sw, sh, data));
        }
        Selection::Region {
            width,
            height,
            mask,
            tight_bbox,
        } => {
            if width != layer.width
                || height != layer.height
                || mask.len() != (width * height) as usize
            {
                return;
            }
            let Some((bx, by, bw, bh)) =
                region_tight_bbox_or_hint(&mask, width, height, tight_bbox)
            else {
                return;
            };
            let data = copy_region_masked(layer, &mask, bx, by, bw, bh);
            clear_region_masked(layer, &mask);
            st.clipboard = Some((bw, bh, data));
        }
    }
    st.history.commit_change(idx, before);
    st.selection = None;
    st.modified = true;
    st.bump_document_revision();
}

fn erase_selection(state: &SharedState) {
    let mut st = state.borrow_mut();
    let Some(sel) = st.selection.clone() else {
        return;
    };
    let idx = st.doc.active_layer;
    let Some(layer) = st.doc.layers.get_mut(idx) else {
        return;
    };
    let before = layer.pixels.clone();
    match sel {
        Selection::Rect(sx, sy, sw, sh) => {
            clear_rect(layer, sx, sy, sw, sh);
        }
        Selection::Region {
            width,
            height,
            mask,
            ..
        } => {
            if width != layer.width
                || height != layer.height
                || mask.len() != (width * height) as usize
            {
                return;
            }
            clear_region_masked(layer, &mask);
        }
    }
    st.history.commit_change(idx, before);
    st.selection = None;
    st.modified = true;
    st.bump_document_revision();
}

fn copy_selection(state: &SharedState) {
    let mut st = state.borrow_mut();
    let Some(sel) = st.selection.clone() else {
        return;
    };
    let Some(layer) = st.doc.active_layer_ref() else {
        return;
    };
    let (sw, sh, data) = match sel {
        Selection::Rect(sx, sy, sw, sh) => {
            let data = copy_rect(layer, sx, sy, sw, sh);
            (sw, sh, data)
        }
        Selection::Region {
            width,
            height,
            mask,
            tight_bbox,
        } => {
            if width != layer.width
                || height != layer.height
                || mask.len() != (width * height) as usize
            {
                return;
            }
            let Some((bx, by, bw, bh)) =
                region_tight_bbox_or_hint(&mask, width, height, tight_bbox)
            else {
                return;
            };
            let data = copy_region_masked(layer, &mask, bx, by, bw, bh);
            (bw, bh, data)
        }
    };
    st.clipboard = Some((sw, sh, data.clone()));

    let straight = premul_to_straight_rgba(&data);
    let bytes = glib::Bytes::from_owned(straight);
    let texture = gdk::MemoryTexture::new(
        sw, sh,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (sw * 4) as usize,
    );
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_texture(&texture);
    }
}

fn paste_clipboard_center(state: &SharedState, tool_dd_cell: &ToolDdCell) {
    let clip = state.borrow().clipboard.clone();
    let Some((sw, sh, data)) = clip else {
        return;
    };
    let mut st = state.borrow_mut();
    commit_floating(&mut st);
    let w = st.doc.width as i32;
    let h = st.doc.height as i32;
    let x = ((w - sw) / 2) as f64;
    let y = ((h - sh) / 2) as f64;
    st.floating = Some(FloatingSelection::new_pasted(sw, sh, data, x, y));
    st.selection = None;
    st.tool = ToolKind::Move;
    drop(st);
    if let Some(ref dd) = *tool_dd_cell.borrow() {
        dd.set_selected(10);
    }
}

fn paste_image_data(
    state: &SharedState,
    tool_dd_cell: &ToolDdCell,
    w: u32,
    h: u32,
    premul_data: Vec<u8>,
) {
    let mut st = state.borrow_mut();
    commit_floating(&mut st);
    let doc_w = st.doc.width as i32;
    let doc_h = st.doc.height as i32;
    let x = ((doc_w - w as i32) / 2).max(0) as f64;
    let y = ((doc_h - h as i32) / 2).max(0) as f64;
    st.floating = Some(FloatingSelection::new_pasted(
        w as i32,
        h as i32,
        premul_data,
        x,
        y,
    ));
    st.selection = None;
    st.tool = ToolKind::Move;
    drop(st);
    if let Some(ref dd) = *tool_dd_cell.borrow() {
        dd.set_selected(10);
    }
}

fn show_paste_oversize_dialog(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    tool_dd_cell: &ToolDdCell,
    canvas_cell: &CanvasCell,
    layers_cell: &LayersCell,
    img_w: u32,
    img_h: u32,
    premul_data: Vec<u8>,
) {
    let doc_w = state.borrow().doc.width;
    let doc_h = state.borrow().doc.height;
    let data = Rc::new(premul_data);

    let d = libadwaita::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Paste image")
        .default_width(400)
        .default_height(160)
        .build();

    let msg = gtk::Label::new(Some(&format!(
        "Image ({img_w}×{img_h}) is larger than the canvas ({doc_w}×{doc_h})."
    )));
    msg.set_wrap(true);

    let cancel = gtk::Button::with_label("Cancel");
    let paste_anyway = gtk::Button::with_label("Paste anyway");
    let expand = gtk::Button::with_label("Expand canvas");
    expand.add_css_class("suggested-action");

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    btn_row.append(&cancel);
    btn_row.append(&paste_anyway);
    btn_row.append(&expand);

    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .spacing(16)
        .build();
    bx.append(&msg);
    bx.append(&btn_row);
    d.set_content(Some(&bx));

    let dw = d.clone();
    cancel.connect_clicked(move |_| dw.close());

    let st = state.clone();
    let td = tool_dd_cell.clone();
    let cv = canvas_cell.clone();
    let data_c = data.clone();
    let dw = d.clone();
    paste_anyway.connect_clicked(move |_| {
        paste_image_data(&st, &td, img_w, img_h, (*data_c).clone());
        queue_canvas(&cv);
        dw.close();
    });

    let st = state.clone();
    let td = tool_dd_cell.clone();
    let cv = canvas_cell.clone();
    let lc = layers_cell.clone();
    let dw = d.clone();
    expand.connect_clicked(move |_| {
        {
            let mut s = st.borrow_mut();
            let new_w = s.doc.width.max(img_w);
            let new_h = s.doc.height.max(img_h);
            s.doc.resize_canvas(new_w, new_h);
            s.history.clear();
            s.bump_document_revision();
        }
        paste_image_data(&st, &td, img_w, img_h, (*data).clone());
        zoom_to_fit(&st, &cv);
        refresh_layers_list(&st, &lc, &cv);
        queue_canvas(&cv);
        dw.close();
    });

    d.present();
}

fn try_paste_system_clipboard(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    tool_dd_cell: &ToolDdCell,
    canvas_cell: &CanvasCell,
    layers_cell: &LayersCell,
) {
    let Some(display) = gdk::Display::default() else {
        paste_clipboard_center(state, tool_dd_cell);
        queue_canvas(canvas_cell);
        return;
    };
    let clipboard = display.clipboard();

    let win = window.clone();
    let st = state.clone();
    let td = tool_dd_cell.clone();
    let cv = canvas_cell.clone();
    let lc = layers_cell.clone();

    clipboard.read_texture_async(
        None::<&gio::Cancellable>,
        move |result| {
            match result {
                Ok(Some(texture)) => {
                    let png_bytes = texture.save_to_png_bytes();
                    match image::load_from_memory(&png_bytes) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let (iw, ih) = rgba.dimensions();
                            let premul = straight_to_premul(rgba.as_raw());

                            let doc_w = st.borrow().doc.width;
                            let doc_h = st.borrow().doc.height;

                            if iw > doc_w || ih > doc_h {
                                show_paste_oversize_dialog(
                                    &win, &st, &td, &cv, &lc, iw, ih, premul,
                                );
                            } else {
                                paste_image_data(&st, &td, iw, ih, premul);
                                queue_canvas(&cv);
                            }
                        }
                        Err(_) => {
                            paste_clipboard_center(&st, &td);
                            queue_canvas(&cv);
                        }
                    }
                }
                _ => {
                    paste_clipboard_center(&st, &td);
                    queue_canvas(&cv);
                }
            }
        },
    );
}

fn recent_file_menu_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn refresh_recent_files_menu(menu: &gio::Menu, state: &AppState) {
    while menu.n_items() > 0 {
        menu.remove(0);
    }
    for path in &state.recent_files {
        let label = recent_file_menu_label(path);
        let item = gio::MenuItem::new(Some(&label), None);
        let ps = path.to_string_lossy();
        let v = ps.as_ref().to_variant();
        item.set_action_and_target_value(Some("win.open_recent"), Some(&v));
        menu.append_item(&item);
    }
}

fn open_document_from_path(
    path: &Path,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
    recent_menu: &gio::Menu,
) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let result = if ext.as_deref() == Some("ora") {
        Document::load_ora(path)
    } else {
        Document::load_raster_image(path)
    };
    match result {
        Ok(doc) => {
            let path_buf = path.to_path_buf();
            let mut g = state.borrow_mut();
            g.doc = doc;
            g.history.clear();
            g.doc.path = Some(path_buf.clone());
            g.modified = false;
            g.selection = None;
            g.floating = None;
            g.floating_drag = None;
            g.shape_drag_preview = None;
            g.drag_start_doc = None;
            g.bump_document_revision();
            crate::settings::record_recent_open(&mut g, path_buf);
            drop(g);
            refresh_recent_files_menu(recent_menu, &state.borrow());
            zoom_to_fit(state, canvas);
            refresh_layers_list(state, layers_cell, canvas);
            queue_canvas(canvas);
        }
        Err(e) => {
            eprintln!("Open failed: {e}");
        }
    }
}

fn open_file(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
    recent_menu: &gio::Menu,
) {
    let all_filter = gtk::FileFilter::new();
    all_filter.set_name(Some("All supported images"));
    for pat in [
        "*.ora", "*.png", "*.jpg", "*.jpeg", "*.jpe", "*.webp", "*.gif", "*.bmp",
    ] {
        all_filter.add_pattern(pat);
    }
    let png_filter = gtk::FileFilter::new();
    png_filter.set_name(Some("PNG (*.png)"));
    png_filter.add_pattern("*.png");
    let ora_filter = gtk::FileFilter::new();
    ora_filter.set_name(Some("OpenRaster (*.ora)"));
    ora_filter.add_pattern("*.ora");
    let jpeg_filter = gtk::FileFilter::new();
    jpeg_filter.set_name(Some("JPEG (*.jpg, *.jpeg)"));
    for pat in ["*.jpg", "*.jpeg", "*.jpe"] {
        jpeg_filter.add_pattern(pat);
    }
    let webp_filter = gtk::FileFilter::new();
    webp_filter.set_name(Some("WebP (*.webp)"));
    webp_filter.add_pattern("*.webp");

    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&all_filter);
    filters.append(&png_filter);
    filters.append(&jpeg_filter);
    filters.append(&webp_filter);
    filters.append(&ora_filter);

    let initial_folder = {
        let g = state.borrow();
        file_dialog_initial_folder(g.doc.path.as_deref(), &g.recent_files)
    };
    let dlg = gtk::FileDialog::builder()
        .title("Open image")
        .modal(true)
        .filters(&filters)
        .default_filter(&all_filter)
        .initial_folder(&initial_folder)
        .build();
    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let recent_m = recent_menu.clone();
    dlg.open(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            if let Some(path) = file.path() {
                open_document_from_path(&path, &st, &lc, &cv, &recent_m);
            }
        }
    });
}

/// Save to the document's current path. Returns `false` if there is no path or the write fails.
fn try_save_to_current_path(state: &SharedState) -> bool {
    let Some(path) = state.borrow().doc.path.clone() else {
        return false;
    };
    let mut g = state.borrow_mut();
    let result = match path.extension().and_then(|e| e.to_str()) {
        Some("ora") => g.doc.save_ora(&path),
        _ => g.doc.save_png(&path),
    };
    if let Err(e) = result {
        eprintln!("Save failed: {e}");
        false
    } else {
        g.modified = false;
        true
    }
}

fn save_file_as(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
    on_success: Option<Rc<dyn Fn()>>,
) {
    let png_filter = gtk::FileFilter::new();
    png_filter.set_name(Some("PNG image (*.png)"));
    png_filter.add_pattern("*.png");
    let ora_filter = gtk::FileFilter::new();
    ora_filter.set_name(Some("OpenRaster (*.ora)"));
    ora_filter.add_pattern("*.ora");

    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&png_filter);
    filters.append(&ora_filter);

    let initial_folder = {
        let g = state.borrow();
        file_dialog_initial_folder(g.doc.path.as_deref(), &g.recent_files)
    };
    let dlg = gtk::FileDialog::builder()
        .title("Save image")
        .modal(true)
        .filters(&filters)
        .default_filter(&png_filter)
        .initial_folder(&initial_folder)
        .build();
    let st = state.clone();
    let cv = canvas.clone();
    let _lc = layers_cell.clone();
    let on_ok = on_success.clone();
    dlg.save(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            if let Some(mut path) = file.path() {
                if path.extension().is_none() {
                    path.set_extension("png");
                }
                let mut g = st.borrow_mut();
                let result = match path.extension().and_then(|e| e.to_str()) {
                    Some("ora") => g.doc.save_ora(&path),
                    _ => g.doc.save_png(&path),
                };
                if let Err(e) = result {
                    eprintln!("Save failed: {e}");
                } else {
                    g.doc.path = Some(path);
                    g.modified = false;
                    if let Some(cb) = &on_ok {
                        cb();
                    }
                }
                drop(g);
                queue_canvas(&cv);
            }
        }
    });
}

fn save_file(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
) {
    if state.borrow().doc.path.is_some() {
        let _ = try_save_to_current_path(state);
    } else {
        save_file_as(window, state, layers_cell, canvas, None);
    }
}

fn new_document_dialog(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
) {
    let d = libadwaita::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("New image")
        .default_width(320)
        .default_height(200)
        .build();

    let w_adj = gtk::Adjustment::new(800.0, 1.0, 8192.0, 1.0, 64.0, 0.0);
    let h_adj = gtk::Adjustment::new(600.0, 1.0, 8192.0, 1.0, 64.0, 0.0);
    let w_spin = gtk::SpinButton::new(Some(&w_adj), 1.0, 0);
    let h_spin = gtk::SpinButton::new(Some(&h_adj), 1.0, 0);

    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .spacing(12)
        .build();
    bx.append(&gtk::Label::new(Some("Width")));
    bx.append(&w_spin);
    bx.append(&gtk::Label::new(Some("Height")));
    bx.append(&h_spin);

    let ok = gtk::Button::with_label("Create");
    let cancel = gtk::Button::with_label("Cancel");
    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    btn_row.append(&cancel);
    btn_row.append(&ok);
    bx.append(&btn_row);

    d.set_content(Some(&bx));

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let dw = d.clone();
    ok.connect_clicked(move |_| {
        let w = w_adj.value() as u32;
        let h = h_adj.value() as u32;
        let mut g = st.borrow_mut();
        g.doc = Document::new(w, h);
        g.history.clear();
        g.selection = None;
        g.floating = None;
        g.floating_drag = None;
        g.shape_drag_preview = None;
        g.drag_start_doc = None;
        g.modified = false;
        g.bump_document_revision();
        drop(g);
        zoom_to_fit(&st, &cv);
        refresh_layers_list(&st, &lc, &cv);
        queue_canvas(&cv);
        dw.close();
    });
    let dw_cancel = d.clone();
    cancel.connect_clicked(move |_| {
        dw_cancel.close();
    });

    d.present();
}

fn refresh_after_canvas_change(
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
    zoom_fit: bool,
) {
    if zoom_fit {
        zoom_to_fit(state, canvas);
    }
    refresh_layers_list(state, layers_cell, canvas);
    queue_canvas(canvas);
}

fn finalize_canvas_geometry_change(g: &mut AppState) {
    g.history.clear();
    g.selection = None;
    g.shape_drag_preview = None;
    g.drag_start_doc = None;
    g.last_doc_pos = None;
    g.move_grab_doc = None;
    g.floating_drag = None;
    g.modified = true;
    g.bump_document_revision();
}

fn canvas_resize_dialog(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
) {
    let d = libadwaita::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Resize canvas")
        .default_width(320)
        .default_height(200)
        .build();

    let w_adj = gtk::Adjustment::new(800.0, 1.0, 8192.0, 1.0, 64.0, 0.0);
    let h_adj = gtk::Adjustment::new(600.0, 1.0, 8192.0, 1.0, 64.0, 0.0);
    let w_spin = gtk::SpinButton::new(Some(&w_adj), 1.0, 0);
    let h_spin = gtk::SpinButton::new(Some(&h_adj), 1.0, 0);
    {
        let st = state.borrow();
        w_adj.set_value(st.doc.width as f64);
        h_adj.set_value(st.doc.height as f64);
    }

    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .spacing(12)
        .build();
    bx.append(&gtk::Label::new(Some("Width")));
    bx.append(&w_spin);
    bx.append(&gtk::Label::new(Some("Height")));
    bx.append(&h_spin);

    let ok = gtk::Button::with_label("Resize");
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    btn_row.append(&cancel);
    btn_row.append(&ok);
    bx.append(&btn_row);
    d.set_content(Some(&bx));

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let dw = d.clone();
    ok.connect_clicked(move |_| {
        let nw = w_adj.value() as u32;
        let nh = h_adj.value() as u32;
        let mut g = st.borrow_mut();
        commit_floating(&mut g);
        let resized = nw != g.doc.width || nh != g.doc.height;
        if resized {
            g.doc.resize_canvas(nw, nh);
            finalize_canvas_geometry_change(&mut g);
        }
        drop(g);
        if resized {
            refresh_after_canvas_change(&st, &lc, &cv, true);
        } else {
            refresh_layers_list(&st, &lc, &cv);
            queue_canvas(&cv);
        }
        dw.close();
    });
    let dw_cancel = d.clone();
    cancel.connect_clicked(move |_| {
        dw_cancel.close();
    });

    d.present();
}

fn keybinds_dialog(window: &libadwaita::ApplicationWindow, state: &SharedState, tool_strings: &gtk::StringList) {
    let d = libadwaita::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Keybinds")
        .default_width(340)
        .default_height(500)
        .build();

    let original_binds = state.borrow().tool_keybinds.clone();
    let saved: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let editing_idx: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    let buttons: Rc<RefCell<Vec<gtk::Button>>> = Rc::new(RefCell::new(Vec::new()));

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");

    fn key_label(k: Option<char>) -> String {
        k.map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_else(|| "None".into())
    }

    let binds = state.borrow().tool_keybinds.clone();
    for (i, (tool, key)) in binds.iter().enumerate() {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        let label = gtk::Label::builder()
            .label(tool.display_name())
            .xalign(0.0)
            .hexpand(true)
            .build();
        let btn = gtk::Button::with_label(&key_label(*key));
        btn.set_width_request(80);

        let ei = editing_idx.clone();
        let btns = buttons.clone();
        btn.connect_clicked(move |b| {
            if let Some(prev) = *ei.borrow() {
                let bs = btns.borrow();
                if let Some(pb) = bs.get(prev) {
                    pb.remove_css_class("suggested-action");
                }
            }
            *ei.borrow_mut() = Some(i);
            b.set_label("…");
            b.add_css_class("suggested-action");
        });

        row.append(&label);
        row.append(&btn);
        list.append(&row);
        buttons.borrow_mut().push(btn);
    }

    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .build();
    scroll.set_child(Some(&list));

    let reset_btn = gtk::Button::with_label("Reset defaults");
    let discard_btn = gtk::Button::with_label("Don't save");
    let save_btn = gtk::Button::with_label("Save");
    save_btn.add_css_class("suggested-action");
    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(8)
        .build();
    btn_row.append(&reset_btn);
    btn_row.append(&discard_btn);
    btn_row.append(&save_btn);

    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .spacing(8)
        .build();
    bx.append(&scroll);
    bx.append(&btn_row);
    d.set_content(Some(&bx));

    let key_ctrl = gtk::EventControllerKey::new();
    let st_key = state.clone();
    let ei_key = editing_idx.clone();
    let btns_key = buttons.clone();
    key_ctrl.connect_key_pressed(move |_, keyval, _, modifier| {
        let Some(idx) = *ei_key.borrow() else {
            return glib::Propagation::Proceed;
        };

        if keyval == gdk::Key::Escape {
            let st = st_key.borrow();
            let text = key_label(st.tool_keybinds[idx].1);
            drop(st);
            let bs = btns_key.borrow();
            if let Some(btn) = bs.get(idx) {
                btn.set_label(&text);
                btn.remove_css_class("suggested-action");
            }
            *ei_key.borrow_mut() = None;
            return glib::Propagation::Stop;
        }

        if keyval == gdk::Key::Delete || keyval == gdk::Key::BackSpace {
            st_key.borrow_mut().tool_keybinds[idx].1 = None;
            let bs = btns_key.borrow();
            if let Some(btn) = bs.get(idx) {
                btn.set_label("None");
                btn.remove_css_class("suggested-action");
            }
            *ei_key.borrow_mut() = None;
            return glib::Propagation::Stop;
        }

        if modifier.contains(gdk::ModifierType::CONTROL_MASK)
            || modifier.contains(gdk::ModifierType::ALT_MASK)
        {
            return glib::Propagation::Proceed;
        }

        let Some(ch) = keyval.to_unicode().map(|c| c.to_ascii_lowercase()) else {
            return glib::Propagation::Proceed;
        };
        if ch.is_control() || ch.is_whitespace() {
            return glib::Propagation::Proceed;
        }

        let mut st = st_key.borrow_mut();
        let bs = btns_key.borrow();
        for (j, (_, bind)) in st.tool_keybinds.iter_mut().enumerate() {
            if *bind == Some(ch) {
                *bind = None;
                if let Some(btn) = bs.get(j) {
                    btn.set_label("None");
                }
            }
        }
        st.tool_keybinds[idx].1 = Some(ch);
        if let Some(btn) = bs.get(idx) {
            btn.set_label(&ch.to_ascii_uppercase().to_string());
            btn.remove_css_class("suggested-action");
        }
        *ei_key.borrow_mut() = None;
        glib::Propagation::Stop
    });
    d.add_controller(key_ctrl);

    let st_reset = state.clone();
    let btns_reset = buttons.clone();
    let ei_reset = editing_idx.clone();
    reset_btn.connect_clicked(move |_| {
        let defaults = AppState::default_tool_keybinds();
        let mut st = st_reset.borrow_mut();
        st.tool_keybinds = defaults.clone();
        let bs = btns_reset.borrow();
        for (i, (_, k)) in defaults.iter().enumerate() {
            if let Some(btn) = bs.get(i) {
                btn.set_label(&key_label(*k));
                btn.remove_css_class("suggested-action");
            }
        }
        *ei_reset.borrow_mut() = None;
    });

    let saved_s = saved.clone();
    let dw = d.clone();
    save_btn.connect_clicked(move |_| {
        *saved_s.borrow_mut() = true;
        dw.close();
    });

    let dw = d.clone();
    let st_dis = state.clone();
    let orig_dis = original_binds.clone();
    discard_btn.connect_clicked(move |_| {
        st_dis.borrow_mut().tool_keybinds = orig_dis.clone();
        dw.close();
    });

    let st_close = state.clone();
    let sl_close = tool_strings.clone();
    let orig_close = original_binds.clone();
    let saved_c = saved.clone();
    d.connect_close_request(move |_| {
        if !*saved_c.borrow() {
            st_close.borrow_mut().tool_keybinds = orig_close.clone();
        } else {
            crate::settings::persist(&st_close.borrow());
        }
        refresh_tool_labels(&st_close, &sl_close);
        glib::Propagation::Proceed
    });

    d.present();
}

fn build_ui(app: &Application) {
    let window = libadwaita::ApplicationWindow::builder()
        .application(app)
        .title("Wooly Paint")
        .default_width(1100)
        .default_height(750)
        .build();

    let win_for_icon = window.clone();
    window.connect_realize(move |_| apply_taskbar_icon(&win_for_icon));

    let mut initial_state = AppState::new();
    let loaded_theme = crate::settings::load_into(&mut initial_state);
    libadwaita::StyleManager::default().set_color_scheme(color_scheme_from_menu_value(loaded_theme));
    let state: SharedState = Rc::new(RefCell::new(initial_state));
    let recent_files_menu = Rc::new(gio::Menu::new());
    refresh_recent_files_menu(&recent_files_menu, &state.borrow());

    let st_shutdown = state.clone();
    app.connect_shutdown(move |_| {
        crate::settings::persist(&st_shutdown.borrow());
    });
    let canvas_cell: CanvasCell = Rc::new(RefCell::new(None));
    let layers_cell: LayersCell = Rc::new(RefCell::new(None));
    let color_preview_da_cell: ColorPreviewDaCell = Rc::new(RefCell::new(None));
    let tool_dd_cell: ToolDdCell = Rc::new(RefCell::new(None));

    let drawing_area = gtk::DrawingArea::new();
    drawing_area.set_hexpand(true);
    drawing_area.set_vexpand(true);
    drawing_area.set_can_focus(true);
    drawing_area.set_focusable(true);
    *canvas_cell.borrow_mut() = Some(drawing_area.clone());

    let st_draw = state.clone();
    drawing_area.set_draw_func(move |_area, cr, _w, _h| {
        draw_canvas(&st_draw, cr);
    });

    let tick_slot: Rc<RefCell<Option<gtk::TickCallbackId>>> = Rc::new(RefCell::new(None));

    let recent_colors_flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(false)
        .max_children_per_line(4)
        .row_spacing(2)
        .column_spacing(2)
        .hexpand(true)
        .halign(gtk::Align::Start)
        .build();

    let picker_ui_refresh: PickerUiRefresh = Rc::new(RefCell::new(None));
    let picker_suppress = Rc::new(Cell::new(false));
    let hue_suppress = Rc::new(Cell::new(false));
    let picker_hue_adj = gtk::Adjustment::new(0.0, 0.0, 360.0, 1.0, 10.0, 0.0);

    let init_slot_color = {
        let st = state.borrow();
        match st.picker_target {
            ColorSlot::Left => st.fg,
            ColorSlot::Right => st.bg,
        }
    };
    let (init_h, init_s, init_v) = rgb_bytes_to_hsv(init_slot_color);
    {
        let mut g = state.borrow_mut();
        g.picker_hue = init_h.rem_euclid(1.0);
    }

    let picker_sat_adj = gtk::Adjustment::new(init_s, 0.0, 1.0, 0.01, 0.05, 0.0);
    let picker_val_adj = gtk::Adjustment::new(init_v, 0.0, 1.0, 0.01, 0.05, 0.0);
    let picker_r_adj =
        gtk::Adjustment::new(init_slot_color[0] as f64, 0.0, 255.0, 1.0, 16.0, 0.0);
    let picker_g_adj =
        gtk::Adjustment::new(init_slot_color[1] as f64, 0.0, 255.0, 1.0, 16.0, 0.0);
    let picker_b_adj =
        gtk::Adjustment::new(init_slot_color[2] as f64, 0.0, 255.0, 1.0, 16.0, 0.0);
    let picker_tracks: Rc<RefCell<Vec<gtk::DrawingArea>>> = Rc::new(RefCell::new(Vec::new()));
    let picker_a_adj =
        gtk::Adjustment::new(init_slot_color[3] as f64, 0.0, 255.0, 1.0, 16.0, 0.0);
    let picker_s_disp_adj =
        gtk::Adjustment::new((init_s * 100.0).round(), 0.0, 100.0, 1.0, 10.0, 0.0);
    let picker_v_disp_adj =
        gtk::Adjustment::new((init_v * 100.0).round(), 0.0, 100.0, 1.0, 10.0, 0.0);

    hue_suppress.set(true);
    picker_hue_adj.set_value(init_h.rem_euclid(1.0) * 360.0);
    hue_suppress.set(false);

    let hex_entry = gtk::Entry::builder()
        .hexpand(true)
        .placeholder_text("#RRGGBB or RRGGBB")
        .build();

    let sv_da = gtk::DrawingArea::builder()
        .width_request(168)
        .height_request(140)
        .hexpand(true)
        .vexpand(false)
        .build();
    let st_svd = state.clone();
    sv_da.set_draw_func(move |_d, cr, w, h| {
        let (hh, c) = {
            let st = st_svd.borrow();
            let c = match st.picker_target {
                ColorSlot::Left => st.fg,
                ColorSlot::Right => st.bg,
            };
            (st.picker_hue, c)
        };
        let w = w as i32;
        let h = h as i32;
        if w <= 0 || h <= 0 {
            return;
        }
        for py in 0..h {
            let v = 1.0 - (py as f64 + 0.5) / h as f64;
            for px in 0..w {
                let s = (px as f64 + 0.5) / w as f64;
                let (r, g, b) = hsv_to_rgb01(hh, s, v);
                cr.set_source_rgb(r, g, b);
                cr.rectangle(px as f64, py as f64, 1.0, 1.0);
                let _ = cr.fill();
            }
        }
        let wf = w as f64;
        let hf = h as f64;
        let (ms, mv) = sv_on_hue_plane_for_rgb(hh, c);
        let cx = (ms * wf).clamp(0.0, wf);
        let cy = ((1.0 - mv) * hf).clamp(0.0, hf);
        const PI: f64 = std::f64::consts::PI;
        let rad = 3.5;
        cr.set_line_width(1.0);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.arc(cx, cy, rad + 0.5, 0.0, 2.0 * PI);
        let _ = cr.stroke();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
        let _ = cr.stroke();
    });

    let fg_bg_da = make_fg_bg_selector(
        &state,
        &canvas_cell,
        &sv_da,
        &picker_ui_refresh,
    );
    *color_preview_da_cell.borrow_mut() = Some(fg_bg_da.clone());

    let da_h = make_picker_track(PickerTrackKind::Hue, &state, &picker_hue_adj, &picker_tracks);
    let da_s = make_picker_track(PickerTrackKind::Sat, &state, &picker_sat_adj, &picker_tracks);
    let da_v = make_picker_track(PickerTrackKind::Val, &state, &picker_val_adj, &picker_tracks);
    let da_r = make_picker_track(PickerTrackKind::Red, &state, &picker_r_adj, &picker_tracks);
    let da_g = make_picker_track(PickerTrackKind::Green, &state, &picker_g_adj, &picker_tracks);
    let da_b = make_picker_track(PickerTrackKind::Blue, &state, &picker_b_adj, &picker_tracks);
    let da_a = make_picker_track(PickerTrackKind::Alpha, &state, &picker_a_adj, &picker_tracks);

    let spin_h = gtk::SpinButton::new(Some(&picker_hue_adj), 1.0, 0);
    spin_h.set_digits(0);
    let spin_s = gtk::SpinButton::new(Some(&picker_s_disp_adj), 1.0, 0);
    let spin_v = gtk::SpinButton::new(Some(&picker_v_disp_adj), 1.0, 0);
    let spin_r = gtk::SpinButton::new(Some(&picker_r_adj), 1.0, 0);
    let spin_g = gtk::SpinButton::new(Some(&picker_g_adj), 1.0, 0);
    let spin_b = gtk::SpinButton::new(Some(&picker_b_adj), 1.0, 0);
    let spin_a = gtk::SpinButton::new(Some(&picker_a_adj), 1.0, 0);

    let hsv_panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    hsv_panel.append(&picker_gradient_spin_row("H:", &da_h, &spin_h));
    hsv_panel.append(&picker_gradient_spin_row("S:", &da_s, &spin_s));
    hsv_panel.append(&picker_gradient_spin_row("V:", &da_v, &spin_v));

    let rgb_panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    rgb_panel.append(&picker_gradient_spin_row("R:", &da_r, &spin_r));
    rgb_panel.append(&picker_gradient_spin_row("G:", &da_g, &spin_g));
    rgb_panel.append(&picker_gradient_spin_row("B:", &da_b, &spin_b));
    rgb_panel.set_visible(false);

    let alpha_lbl = gtk::Label::new(Some("Alpha"));
    alpha_lbl.add_css_class("dim-label");
    alpha_lbl.set_halign(gtk::Align::Start);
    let alpha_panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    alpha_panel.append(&alpha_lbl);
    alpha_panel.append(&picker_gradient_spin_row("A:", &da_a, &spin_a));

    let hsv_toggle = gtk::ToggleButton::with_label("HSV");
    let rgb_toggle = gtk::ToggleButton::with_label("RGB");
    rgb_toggle.set_group(Some(&hsv_toggle));
    hsv_toggle.set_active(true);
    let mode_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .hexpand(true)
        .build();
    mode_row.add_css_class("linked");
    mode_row.append(&hsv_toggle);
    mode_row.append(&rgb_toggle);

    let hsv_panel_vis = hsv_panel.clone();
    let rgb_panel_vis = rgb_panel.clone();
    hsv_toggle.connect_toggled(move |t| {
        if t.is_active() {
            hsv_panel_vis.set_visible(true);
            rgb_panel_vis.set_visible(false);
        }
    });
    let hsv_panel_vis2 = hsv_panel.clone();
    let rgb_panel_vis2 = rgb_panel.clone();
    rgb_toggle.connect_toggled(move |t| {
        if t.is_active() {
            hsv_panel_vis2.set_visible(false);
            rgb_panel_vis2.set_visible(true);
        }
    });

    let ps_sd = picker_suppress.clone();
    let sat_core = picker_sat_adj.clone();
    picker_s_disp_adj.connect_value_changed(move |adj| {
        if ps_sd.get() {
            return;
        }
        sat_core.set_value((adj.value() / 100.0).clamp(0.0, 1.0));
    });
    let ps_vd = picker_suppress.clone();
    let val_core = picker_val_adj.clone();
    picker_v_disp_adj.connect_value_changed(move |adj| {
        if ps_vd.get() {
            return;
        }
        val_core.set_value((adj.value() / 100.0).clamp(0.0, 1.0));
    });

    let st_hue = state.clone();
    let sv_hue = sv_da.clone();
    let fg_hue = fg_bg_da.clone();
    let cv_hue = canvas_cell.clone();
    let sup_hue = hue_suppress.clone();
    let ps_hue = picker_suppress.clone();
    let hue_adj_h = picker_hue_adj.clone();
    let sat_h = picker_sat_adj.clone();
    let val_h = picker_val_adj.clone();
    let rh = picker_r_adj.clone();
    let gh = picker_g_adj.clone();
    let bh = picker_b_adj.clone();
    let ah = picker_a_adj.clone();
    let sd_h = picker_s_disp_adj.clone();
    let vd_h = picker_v_disp_adj.clone();
    let tr_h = picker_tracks.clone();
    let hex_h = hex_entry.clone();
    picker_hue_adj.connect_value_changed(move |adj| {
        if sup_hue.get() {
            return;
        }
        let mut g = st_hue.borrow_mut();
        g.picker_hue = adj.value() / 360.0;
        let slot = g.picker_target;
        let s = sat_h.value().clamp(0.0, 1.0);
        let v = val_h.value().clamp(0.0, 1.0);
        let rgb = hsv_to_rgb01(g.picker_hue, s, v);
        let a = ah.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(&mut g, slot, rgb, a);
        drop(g);
        sync_picker_from_target_color(
            &st_hue,
            sup_hue.as_ref(),
            ps_hue.as_ref(),
            &hue_adj_h,
            &sat_h,
            &val_h,
            &rh,
            &gh,
            &bh,
            &ah,
            &sd_h,
            &vd_h,
            &hex_h,
            &tr_h,
            &sv_hue,
        );
        fg_hue.queue_draw();
        queue_canvas(&cv_hue);
    });

    let st_s = state.clone();
    let ps_s = picker_suppress.clone();
    let sup_s = hue_suppress.clone();
    let sv_s = sv_da.clone();
    let fg_s = fg_bg_da.clone();
    let cv_s = canvas_cell.clone();
    let sat_a = picker_sat_adj.clone();
    let val_a = picker_val_adj.clone();
    let hue_a = picker_hue_adj.clone();
    let r_s = picker_r_adj.clone();
    let g_s = picker_g_adj.clone();
    let b_s = picker_b_adj.clone();
    let a_s = picker_a_adj.clone();
    let sd_s = picker_s_disp_adj.clone();
    let vd_s = picker_v_disp_adj.clone();
    let tr_s = picker_tracks.clone();
    let hex_s = hex_entry.clone();
    picker_sat_adj.connect_value_changed(move |adj| {
        if ps_s.get() {
            return;
        }
        let mut g = st_s.borrow_mut();
        let h = g.picker_hue;
        let slot = g.picker_target;
        let s = adj.value().clamp(0.0, 1.0);
        let v = val_a.value().clamp(0.0, 1.0);
        let rgb = hsv_to_rgb01(h, s, v);
        let a = a_s.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(&mut g, slot, rgb, a);
        drop(g);
        sync_picker_from_target_color(
            &st_s,
            sup_s.as_ref(),
            ps_s.as_ref(),
            &hue_a,
            &sat_a,
            &val_a,
            &r_s,
            &g_s,
            &b_s,
            &a_s,
            &sd_s,
            &vd_s,
            &hex_s,
            &tr_s,
            &sv_s,
        );
        fg_s.queue_draw();
        queue_canvas(&cv_s);
    });

    let st_v = state.clone();
    let ps_v = picker_suppress.clone();
    let sup_v = hue_suppress.clone();
    let sv_v = sv_da.clone();
    let fg_v = fg_bg_da.clone();
    let cv_v = canvas_cell.clone();
    let sat_av = picker_sat_adj.clone();
    let val_av = picker_val_adj.clone();
    let hue_av = picker_hue_adj.clone();
    let r_v = picker_r_adj.clone();
    let g_v = picker_g_adj.clone();
    let b_v = picker_b_adj.clone();
    let a_v = picker_a_adj.clone();
    let sd_v = picker_s_disp_adj.clone();
    let vd_v = picker_v_disp_adj.clone();
    let tr_v = picker_tracks.clone();
    let hex_v = hex_entry.clone();
    picker_val_adj.connect_value_changed(move |adj| {
        if ps_v.get() {
            return;
        }
        let mut g = st_v.borrow_mut();
        let h = g.picker_hue;
        let slot = g.picker_target;
        let s = sat_av.value().clamp(0.0, 1.0);
        let v = adj.value().clamp(0.0, 1.0);
        let rgb = hsv_to_rgb01(h, s, v);
        let a = a_v.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(&mut g, slot, rgb, a);
        drop(g);
        sync_picker_from_target_color(
            &st_v,
            sup_v.as_ref(),
            ps_v.as_ref(),
            &hue_av,
            &sat_av,
            &val_av,
            &r_v,
            &g_v,
            &b_v,
            &a_v,
            &sd_v,
            &vd_v,
            &hex_v,
            &tr_v,
            &sv_v,
        );
        fg_v.queue_draw();
        queue_canvas(&cv_v);
    });

    let st_r = state.clone();
    let ps_r = picker_suppress.clone();
    let sup_r = hue_suppress.clone();
    let sv_r = sv_da.clone();
    let fg_r = fg_bg_da.clone();
    let cv_r = canvas_cell.clone();
    let r_ar = picker_r_adj.clone();
    let g_ar = picker_g_adj.clone();
    let b_ar = picker_b_adj.clone();
    let sat_ar = picker_sat_adj.clone();
    let val_ar = picker_val_adj.clone();
    let hue_ar = picker_hue_adj.clone();
    let a_r = picker_a_adj.clone();
    let sd_r = picker_s_disp_adj.clone();
    let vd_r = picker_v_disp_adj.clone();
    let tr_r = picker_tracks.clone();
    let hex_r = hex_entry.clone();
    picker_r_adj.connect_value_changed(move |adj| {
        if ps_r.get() {
            return;
        }
        let mut g = st_r.borrow_mut();
        let slot = g.picker_target;
        let r = adj.value().round().clamp(0.0, 255.0) as u8;
        let gg = g_ar.value().round().clamp(0.0, 255.0) as u8;
        let bb = b_ar.value().round().clamp(0.0, 255.0) as u8;
        let aa = a_r.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(
            &mut g,
            slot,
            (r as f64 / 255.0, gg as f64 / 255.0, bb as f64 / 255.0),
            aa,
        );
        drop(g);
        sync_picker_from_target_color(
            &st_r,
            sup_r.as_ref(),
            ps_r.as_ref(),
            &hue_ar,
            &sat_ar,
            &val_ar,
            &r_ar,
            &g_ar,
            &b_ar,
            &a_r,
            &sd_r,
            &vd_r,
            &hex_r,
            &tr_r,
            &sv_r,
        );
        fg_r.queue_draw();
        queue_canvas(&cv_r);
    });

    let st_g = state.clone();
    let ps_g = picker_suppress.clone();
    let sup_g = hue_suppress.clone();
    let sv_gm = sv_da.clone();
    let fg_g = fg_bg_da.clone();
    let cv_g = canvas_cell.clone();
    let r_ag = picker_r_adj.clone();
    let g_ag = picker_g_adj.clone();
    let b_ag = picker_b_adj.clone();
    let sat_ag = picker_sat_adj.clone();
    let val_ag = picker_val_adj.clone();
    let hue_ag = picker_hue_adj.clone();
    let a_g = picker_a_adj.clone();
    let sd_g = picker_s_disp_adj.clone();
    let vd_g = picker_v_disp_adj.clone();
    let tr_g = picker_tracks.clone();
    let hex_g = hex_entry.clone();
    picker_g_adj.connect_value_changed(move |adj| {
        if ps_g.get() {
            return;
        }
        let mut g = st_g.borrow_mut();
        let slot = g.picker_target;
        let r = r_ag.value().round().clamp(0.0, 255.0) as u8;
        let gg = adj.value().round().clamp(0.0, 255.0) as u8;
        let bb = b_ag.value().round().clamp(0.0, 255.0) as u8;
        let aa = a_g.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(
            &mut g,
            slot,
            (r as f64 / 255.0, gg as f64 / 255.0, bb as f64 / 255.0),
            aa,
        );
        drop(g);
        sync_picker_from_target_color(
            &st_g,
            sup_g.as_ref(),
            ps_g.as_ref(),
            &hue_ag,
            &sat_ag,
            &val_ag,
            &r_ag,
            &g_ag,
            &b_ag,
            &a_g,
            &sd_g,
            &vd_g,
            &hex_g,
            &tr_g,
            &sv_gm,
        );
        fg_g.queue_draw();
        queue_canvas(&cv_g);
    });

    let st_b = state.clone();
    let ps_b = picker_suppress.clone();
    let sup_b = hue_suppress.clone();
    let sv_b = sv_da.clone();
    let fg_b = fg_bg_da.clone();
    let cv_b = canvas_cell.clone();
    let r_ab = picker_r_adj.clone();
    let g_ab = picker_g_adj.clone();
    let b_ab = picker_b_adj.clone();
    let sat_ab = picker_sat_adj.clone();
    let val_ab = picker_val_adj.clone();
    let hue_ab = picker_hue_adj.clone();
    let a_b = picker_a_adj.clone();
    let sd_b = picker_s_disp_adj.clone();
    let vd_b = picker_v_disp_adj.clone();
    let tr_b = picker_tracks.clone();
    let hex_b = hex_entry.clone();
    picker_b_adj.connect_value_changed(move |adj| {
        if ps_b.get() {
            return;
        }
        let mut g = st_b.borrow_mut();
        let slot = g.picker_target;
        let r = r_ab.value().round().clamp(0.0, 255.0) as u8;
        let gg = g_ab.value().round().clamp(0.0, 255.0) as u8;
        let bb = adj.value().round().clamp(0.0, 255.0) as u8;
        let aa = a_b.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(
            &mut g,
            slot,
            (r as f64 / 255.0, gg as f64 / 255.0, bb as f64 / 255.0),
            aa,
        );
        drop(g);
        sync_picker_from_target_color(
            &st_b,
            sup_b.as_ref(),
            ps_b.as_ref(),
            &hue_ab,
            &sat_ab,
            &val_ab,
            &r_ab,
            &g_ab,
            &b_ab,
            &a_b,
            &sd_b,
            &vd_b,
            &hex_b,
            &tr_b,
            &sv_b,
        );
        fg_b.queue_draw();
        queue_canvas(&cv_b);
    });

    let st_alph = state.clone();
    let ps_alph = picker_suppress.clone();
    let sup_alph = hue_suppress.clone();
    let sv_alph = sv_da.clone();
    let fg_alph = fg_bg_da.clone();
    let cv_alph = canvas_cell.clone();
    let hue_alph = picker_hue_adj.clone();
    let sat_alph = picker_sat_adj.clone();
    let val_alph = picker_val_adj.clone();
    let r_alph = picker_r_adj.clone();
    let g_alph = picker_g_adj.clone();
    let b_alph = picker_b_adj.clone();
    let a_alph = picker_a_adj.clone();
    let sd_alph = picker_s_disp_adj.clone();
    let vd_alph = picker_v_disp_adj.clone();
    let tr_alph = picker_tracks.clone();
    let hex_alph = hex_entry.clone();
    picker_a_adj.connect_value_changed(move |adj| {
        if ps_alph.get() {
            return;
        }
        let mut g = st_alph.borrow_mut();
        let slot = g.picker_target;
        let (r0, g0, b0) = match slot {
            ColorSlot::Left => (g.fg[0], g.fg[1], g.fg[2]),
            ColorSlot::Right => (g.bg[0], g.bg[1], g.bg[2]),
        };
        let rgb = (
            r0 as f64 / 255.0,
            g0 as f64 / 255.0,
            b0 as f64 / 255.0,
        );
        let aa = adj.value().round().clamp(0.0, 255.0) as u8;
        write_slot_rgb_a(&mut g, slot, rgb, aa);
        drop(g);
        sync_picker_from_target_color(
            &st_alph,
            sup_alph.as_ref(),
            ps_alph.as_ref(),
            &hue_alph,
            &sat_alph,
            &val_alph,
            &r_alph,
            &g_alph,
            &b_alph,
            &a_alph,
            &sd_alph,
            &vd_alph,
            &hex_alph,
            &tr_alph,
            &sv_alph,
        );
        fg_alph.queue_draw();
        queue_canvas(&cv_alph);
    });

    let st_hex = state.clone();
    let sup_hex = hue_suppress.clone();
    let ps_hex = picker_suppress.clone();
    let sv_hex = sv_da.clone();
    let fg_hex = fg_bg_da.clone();
    let cv_hex = canvas_cell.clone();
    let ha = picker_hue_adj.clone();
    let sa = picker_sat_adj.clone();
    let va = picker_val_adj.clone();
    let ra = picker_r_adj.clone();
    let ga = picker_g_adj.clone();
    let ba = picker_b_adj.clone();
    let aa_hex = picker_a_adj.clone();
    let sd_hex = picker_s_disp_adj.clone();
    let vd_hex = picker_v_disp_adj.clone();
    let tr_hex = picker_tracks.clone();
    let hex_e = hex_entry.clone();
    hex_entry.connect_activate(move |e| {
        let t = e.text().to_string();
        let trimmed = t.trim();
        let Some(mut rgba) = palette::parse_hex_color_input(trimmed) else {
            return;
        };
        let hex_digits: String = trimmed
            .trim_start_matches('#')
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        let use_parsed_alpha = hex_digits.len() == 8;
        let mut g = st_hex.borrow_mut();
        let slot = g.picker_target;
        if !use_parsed_alpha {
            rgba[3] = aa_hex.value().round().clamp(0.0, 255.0) as u8;
        }
        match slot {
            ColorSlot::Left => {
                g.fg = rgba;
                push_recent_color(&mut g, rgba);
            }
            ColorSlot::Right => g.bg = rgba,
        }
        let (h, _, _) = rgb_bytes_to_hsv(rgba);
        g.picker_hue = h.rem_euclid(1.0);
        drop(g);
        sync_picker_from_target_color(
            &st_hex,
            sup_hex.as_ref(),
            ps_hex.as_ref(),
            &ha,
            &sa,
            &va,
            &ra,
            &ga,
            &ba,
            &aa_hex,
            &sd_hex,
            &vd_hex,
            &hex_e,
            &tr_hex,
            &sv_hex,
        );
        fg_hex.queue_draw();
        queue_canvas(&cv_hex);
    });

    let st_ref = state.clone();
    let sup_ref = hue_suppress.clone();
    let ps_ref = picker_suppress.clone();
    let ha_ref = picker_hue_adj.clone();
    let sa_ref = picker_sat_adj.clone();
    let va_ref = picker_val_adj.clone();
    let ra_ref = picker_r_adj.clone();
    let ga_ref = picker_g_adj.clone();
    let ba_ref = picker_b_adj.clone();
    let aa_ref = picker_a_adj.clone();
    let sd_ref = picker_s_disp_adj.clone();
    let vd_ref = picker_v_disp_adj.clone();
    let tr_ref = picker_tracks.clone();
    let hex_ref = hex_entry.clone();
    let sv_ref = sv_da.clone();
    *picker_ui_refresh.borrow_mut() = Some(Rc::new(move || {
        sync_picker_from_target_color(
            &st_ref,
            sup_ref.as_ref(),
            ps_ref.as_ref(),
            &ha_ref,
            &sa_ref,
            &va_ref,
            &ra_ref,
            &ga_ref,
            &ba_ref,
            &aa_ref,
            &sd_ref,
            &vd_ref,
            &hex_ref,
            &tr_ref,
            &sv_ref,
        );
    }));
    picker_refresh_call(&picker_ui_refresh);

    let sv_painting = Rc::new(Cell::new(false));
    let sv_g = gtk::GestureClick::new();
    sv_g.set_button(0);
    let sv_paint_on = sv_painting.clone();
    let st_press = state.clone();
    let sv_press = sv_da.clone();
    let fg_press = fg_bg_da.clone();
    let cv_press = canvas_cell.clone();
    let pr_press = picker_ui_refresh.clone();
    sv_g.connect_pressed(move |gesture, _, x, y| {
        sv_paint_on.set(true);
        {
            let mut g = st_press.borrow_mut();
            g.picker_target = picker_target_for_gesture_button(gesture.current_button());
        }
        apply_sv_pick(
            &st_press,
            &sv_press,
            &fg_press,
            &cv_press,
            &pr_press,
            x,
            y,
        );
    });
    let sv_paint_m = sv_painting.clone();
    let st_m = state.clone();
    let sv_m = sv_da.clone();
    let fg_m = fg_bg_da.clone();
    let cv_m = canvas_cell.clone();
    let pr_m = picker_ui_refresh.clone();
    let sv_motion = gtk::EventControllerMotion::new();
    sv_motion.connect_motion(move |ec, x, y| {
        if !sv_paint_m.get() {
            return;
        }
        let buttons = ec.current_event_state();
        if !buttons.contains(gdk::ModifierType::BUTTON1_MASK)
            && !buttons.contains(gdk::ModifierType::BUTTON3_MASK)
        {
            return;
        }
        apply_sv_pick(&st_m, &sv_m, &fg_m, &cv_m, &pr_m, x, y);
    });
    let sv_paint_off = sv_painting.clone();
    sv_g.connect_released(move |_, _, _, _| {
        sv_paint_off.set(false);
    });
    sv_da.add_controller(sv_g);
    sv_da.add_controller(sv_motion);

    tool_cursors::sync_canvas_tool_cursor(&drawing_area, state.borrow().tool);

    let layers_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Browse)
        .vexpand(true)
        .build();
    layers_list.add_css_class("boxed-list");
    *layers_cell.borrow_mut() = Some(layers_list.clone());

    let st_sel = state.clone();
    let cv_sel = canvas_cell.clone();
    layers_list.connect_row_selected(move |lb, row| {
        let Some(row) = row else { return };
        let idx = listbox_row_index(lb, row);
        let mut g = st_sel.borrow_mut();
        g.doc.active_layer = idx;
        if matches!(g.selection, Some(Selection::Region { .. })) {
            g.selection = None;
        }
        drop(g);
        queue_canvas(&cv_sel);
    });

    refresh_layers_list(&state, &layers_cell, &canvas_cell);

    let layers_sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .width_request(320)
        .build();
    let layers_title = gtk::Label::new(Some("Layers"));
    layers_title.add_css_class("heading");
    layers_sidebar.append(&layers_title);
    let add_layer_btn = gtk::Button::with_label("Add layer");
    let st_al = state.clone();
    let lc_al = layers_cell.clone();
    let cv_al = canvas_cell.clone();
    add_layer_btn.connect_clicked(move |_| {
        let mut g = st_al.borrow_mut();
        g.doc.add_layer();
        g.history.clear();
        g.bump_document_revision();
        drop(g);
        refresh_layers_list(&st_al, &lc_al, &cv_al);
        queue_canvas(&cv_al);
    });
    layers_sidebar.append(&add_layer_btn);
    let layers_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&layers_list)
        .build();
    layers_sidebar.append(&layers_scroll);

    let labels: Vec<String> = state.borrow().tool_keybinds.iter()
        .map(|(t, k)| tool_label(*t, *k)).collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let tool_strings = gtk::StringList::new(&label_refs);
    let tool_dd = gtk::DropDown::new(Some(tool_strings.clone()), gtk::Expression::NONE);
    *tool_dd_cell.borrow_mut() = Some(tool_dd.clone());
    tool_dd.set_hexpand(false);

    let size_adj_cell: Rc<RefCell<Option<gtk::Adjustment>>> = Rc::new(RefCell::new(None));
    let st_dd = state.clone();
    let cv_dd = canvas_cell.clone();
    let sa_dd = size_adj_cell.clone();
    let da_dd = drawing_area.clone();
    tool_dd.connect_selected_item_notify(move |dd| {
        let mut g = st_dd.borrow_mut();
        let prev = g.tool;
        g.tool = match dd.selected() {
            1 => ToolKind::Pixel,
            2 => ToolKind::Eraser,
            3 => ToolKind::Eyedropper,
            4 => ToolKind::Fill,
            5 => ToolKind::Line,
            6 => ToolKind::Rect,
            7 => ToolKind::Ellipse,
            8 => ToolKind::SelectRect,
            9 => ToolKind::MagicSelect,
            10 => ToolKind::Move,
            11 => ToolKind::Hand,
            _ => ToolKind::Brush,
        };
        let cur = g.tool;
        if g.floating.is_some() && cur != ToolKind::Move {
            commit_floating(&mut g);
        }
        drop(g);
        if let Some(ref adj) = *sa_dd.borrow() {
            if cur == ToolKind::Pixel && prev != ToolKind::Pixel {
                adj.set_value(1.0);
            } else if prev == ToolKind::Pixel && cur != ToolKind::Pixel {
                adj.set_value(8.0);
            }
        }
        tool_cursors::sync_canvas_tool_cursor(&da_dd, cur);
        queue_canvas(&cv_dd);
    });

    let size_adj = gtk::Adjustment::new(8.0, 1.0, 256.0, 1.0, 8.0, 0.0);
    *size_adj_cell.borrow_mut() = Some(size_adj.clone());
    let size_spin = gtk::SpinButton::new(Some(&size_adj), 1.0, 0);
    size_spin.set_width_request(72);
    let st_sz = state.clone();
    size_adj.connect_value_changed(move |a| {
        st_sz.borrow_mut().brush_size = a.value();
    });
    let cv_size = canvas_cell.clone();
    let size_key = gtk::EventControllerKey::new();
    size_key.set_propagation_phase(gtk::PropagationPhase::Capture);
    size_key.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
            if let Some(ref da) = *cv_size.borrow() { da.grab_focus(); }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    size_spin.add_controller(size_key);

    // Gtk/Adwaita often maps the adjustment *maximum* to the visual left for horizontal scales.
    // Invert the range (min on the right) and map `hardness = (min+max) - value` so **screen right**
    // = 100% hard and **screen left** = 10%. Default value 1.0 → thumb on the left (10% hardness).
    // LTR keeps left/right unmirrored for RTL desktops.
    let hard_adj = gtk::Adjustment::new(1.0, 0.1, 1.0, 0.01, 0.1, 0.0);
    let hard_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&hard_adj));
    hard_scale.set_direction(gtk::TextDirection::Ltr);
    hard_scale.set_inverted(true);
    hard_scale.set_hexpand(false);
    hard_scale.set_width_request(120);
    hard_scale.set_halign(gtk::Align::Start);
    let st_h = state.clone();
    hard_adj.connect_value_changed(move |a| {
        st_h.borrow_mut().brush_hardness = 1.1 - a.value();
    });

    let tol_adj = gtk::Adjustment::new(32.0, 0.0, 255.0, 1.0, 16.0, 0.0);
    let tol_spin = gtk::SpinButton::new(Some(&tol_adj), 1.0, 0);
    let st_t = state.clone();
    tol_adj.connect_value_changed(move |a| {
        st_t.borrow_mut().fill_tolerance = a.value() as u8;
    });
    let cv_tol = canvas_cell.clone();
    let tol_key = gtk::EventControllerKey::new();
    tol_key.set_propagation_phase(gtk::PropagationPhase::Capture);
    tol_key.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
            if let Some(ref da) = *cv_tol.borrow() { da.grab_focus(); }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    tol_spin.add_controller(tol_key);

    let fill_check = gtk::CheckButton::with_label("Filled shape");
    let st_f = state.clone();
    fill_check.connect_toggled(move |c| {
        st_f.borrow_mut().shape_filled = c.is_active();
    });

    tool_dd.set_width_request(tool_dropdown_width_request(&tool_dd, &labels));
    size_spin.set_width_request(64);
    tol_spin.set_width_request(64);

    let opt_lbl = |text: &'static str| -> gtk::Label {
        let l = gtk::Label::new(Some(text));
        l.add_css_class("dim-label");
        l.set_valign(gtk::Align::Center);
        l
    };
    let vsep = || gtk::Separator::new(gtk::Orientation::Vertical);

    let options_bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(2)
        .margin_bottom(2)
        .margin_start(10)
        .margin_end(10)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();

    tool_dd.set_valign(gtk::Align::Center);
    size_spin.set_valign(gtk::Align::Center);
    hard_scale.set_valign(gtk::Align::Center);
    tol_spin.set_valign(gtk::Align::Center);
    fill_check.set_valign(gtk::Align::Center);

    options_bar.append(&opt_lbl("Tool"));
    options_bar.append(&tool_dd);
    options_bar.append(&vsep());
    options_bar.append(&opt_lbl("Size"));
    options_bar.append(&size_spin);
    options_bar.append(&vsep());
    options_bar.append(&opt_lbl("Hardness"));
    options_bar.append(&hard_scale);
    options_bar.append(&vsep());
    options_bar.append(&opt_lbl("Fill tol"));
    options_bar.append(&tol_spin);
    options_bar.append(&vsep());
    options_bar.append(&fill_check);
    options_bar.append(&vsep());

    let zoom_out_btn = gtk::Button::from_icon_name("zoom-out-symbolic");
    let zoom_fit_btn = gtk::Button::from_icon_name("zoom-fit-best-symbolic");
    let zoom_in_btn = gtk::Button::from_icon_name("zoom-in-symbolic");
    zoom_out_btn.set_tooltip_text(Some("Zoom out (Ctrl+−)"));
    zoom_fit_btn.set_tooltip_text(Some("Zoom to fit (Ctrl+0)"));
    zoom_in_btn.set_tooltip_text(Some("Zoom in (Ctrl++)"));
    zoom_out_btn.set_valign(gtk::Align::Center);
    zoom_fit_btn.set_valign(gtk::Align::Center);
    zoom_in_btn.set_valign(gtk::Align::Center);
    options_bar.append(&zoom_out_btn);
    options_bar.append(&zoom_fit_btn);
    options_bar.append(&zoom_in_btn);

    let st_zo = state.clone();
    let cv_zo = canvas_cell.clone();
    zoom_out_btn.connect_clicked(move |_| {
        zoom_step(&st_zo, &cv_zo, 1.0 / 1.25);
        queue_canvas(&cv_zo);
    });
    let st_zf = state.clone();
    let cv_zf = canvas_cell.clone();
    zoom_fit_btn.connect_clicked(move |_| {
        zoom_to_fit(&st_zf, &cv_zf);
        queue_canvas(&cv_zf);
    });
    let st_zi = state.clone();
    let cv_zi = canvas_cell.clone();
    zoom_in_btn.connect_clicked(move |_| {
        zoom_step(&st_zi, &cv_zi, 1.25);
        queue_canvas(&cv_zi);
    });

    let color_sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(4)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .width_request(212)
        .hexpand(false)
        .vexpand(true)
        .build();

    let palette_strings = gtk::StringList::new(&[]);
    let palette_dd = gtk::DropDown::new(Some(palette_strings.clone()), gtk::Expression::NONE);
    let palette_row_factory = palette_dropdown_row_factory();
    palette_dd.set_factory(Some(&palette_row_factory));
    palette_dd.set_list_factory(Some(&palette_row_factory));
    palette_dd.set_hexpand(true);
    palette_dd.set_halign(gtk::Align::Fill);
    palette_dd.set_valign(gtk::Align::Center);

    let palette_top = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    palette_top.append(&fg_bg_da);
    palette_top.append(&palette_dd);
    color_sidebar.append(&palette_top);

    let palette_flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .max_children_per_line(4)
        .row_spacing(0)
        .column_spacing(0)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Start)
        .build();
    let palette_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .max_content_height(520)
               .child(&palette_flow)
        .build();
    color_sidebar.append(&palette_scroll);

    color_sidebar.append(&mode_row);
    color_sidebar.append(&hsv_panel);
    color_sidebar.append(&rgb_panel);
    color_sidebar.append(&alpha_panel);
    let hex_lbl = gtk::Label::new(Some("Hex"));
    hex_lbl.add_css_class("dim-label");
    hex_lbl.set_halign(gtk::Align::Start);
    color_sidebar.append(&hex_lbl);
    color_sidebar.append(&hex_entry);
    color_sidebar.append(&sv_da);

    let palette_sidebar = PaletteSidebar {
        flow: palette_flow.clone(),
        dropdown: palette_dd.clone(),
        strings: palette_strings.clone(),
        preview: fg_bg_da.clone(),
        recent: recent_colors_flow.clone(),
        canvas: canvas_cell.clone(),
        sv_area: sv_da.clone(),
        picker_refresh: picker_ui_refresh.clone(),
    };
    refresh_palette_sidebar_full(&palette_sidebar, &state);

    let st_pdd = state.clone();
    let psc_pal = palette_sidebar.clone();
    palette_dd.connect_selected_notify(move |dd| {
        let i = dd.selected() as usize;
        let n = st_pdd.borrow().palette_book.entries.len();
        if i == n {
            {
                let mut g = st_pdd.borrow_mut();
                g.palette_book.new_empty_swatch();
            }
            crate::settings::persist(&st_pdd.borrow());
            refresh_palette_sidebar_full(&psc_pal, &st_pdd);
            return;
        }
        let mut g = st_pdd.borrow_mut();
        if i >= n {
            return;
        }
        if g.palette_book.active == i {
            return;
        }
        g.palette_book.active = i;
        drop(g);
        crate::settings::persist(&st_pdd.borrow());
        fill_palette_swatches(&psc_pal, &st_pdd);
    });

    let recent_label = gtk::Label::builder()
        .label("Last used")
        .xalign(0.0)
        .margin_top(4)
        .halign(gtk::Align::Start)
        .hexpand(false)
        .build();
    recent_label.add_css_class("dim-label");
    color_sidebar.append(&recent_label);
    color_sidebar.append(&recent_colors_flow);

    refresh_recent_swatch_row(
        &recent_colors_flow,
        &state,
        &fg_bg_da,
        &canvas_cell,
        &picker_ui_refresh,
    );

    setup_canvas_input(
        &drawing_area,
        &state,
        &canvas_cell,
        &color_preview_da_cell,
        &recent_colors_flow,
        &picker_ui_refresh,
    );

    let editor = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .vexpand(true)
        .hexpand(true)
        .build();
    editor.append(&color_sidebar);
    editor.append(&drawing_area);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideRight)
        .reveal_child(false)
        .child(&layers_sidebar)
        .build();

    let toggle_layers = gtk::Button::with_label("Layers");
    toggle_layers.set_tooltip_text(Some("Toggle layers panel"));
    toggle_layers.set_valign(gtk::Align::Center);
    let rev_c = revealer.clone();
    toggle_layers.connect_clicked(move |_| {
        rev_c.set_reveal_child(!rev_c.reveals_child());
    });

    let st_tl = state.clone();
    let cv_tl = canvas_cell.clone();
    revealer.connect_child_revealed_notify(move |_| {
        zoom_to_fit(&st_tl, &cv_tl);
        queue_canvas(&cv_tl);
    });

    let split = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .vexpand(true)
        .hexpand(true)
        .build();
    split.append(&editor);
    split.append(&revealer);

    let menu_model = build_menu(
        &window,
        &state,
        &layers_cell,
        &canvas_cell,
        &tool_dd_cell,
        &tool_strings,
        recent_files_menu.clone(),
        palette_sidebar.clone(),
    );
    let menubar = gtk::PopoverMenuBar::from_model(Some(&menu_model));

    let header = libadwaita::HeaderBar::new();
    header.pack_start(&menubar);
    header.pack_end(&toggle_layers);
    let window_title = libadwaita::WindowTitle::new("Wooly Paint", "");
    window_title.set_valign(gtk::Align::Center);
    header.set_title_widget(Some(&window_title));

    let toolbar_view = libadwaita::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.add_top_bar(&options_bar);
    toolbar_view.set_content(Some(&split));
    window.set_content(Some(&toolbar_view));

    let key = gtk::EventControllerKey::new();
    let st_k = state.clone();
    let lc_k = layers_cell.clone();
    let cv_k = canvas_cell.clone();
    let win_k = window.clone();
    let td_k = tool_dd_cell.clone();
    let recent_k = recent_files_menu.clone();
    key.connect_key_pressed(move |_c, keyval, _code, state_m| {
        let ctrl = state_m.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state_m.contains(gdk::ModifierType::SHIFT_MASK);
        match keyval {
            gdk::Key::z | gdk::Key::Z if ctrl && shift => {
                if st_k.borrow_mut().redo() {
                    queue_canvas(&cv_k);
                }
                glib::Propagation::Stop
            }
            gdk::Key::z | gdk::Key::Z if ctrl => {
                if st_k.borrow_mut().undo() {
                    queue_canvas(&cv_k);
                }
                glib::Propagation::Stop
            }
            gdk::Key::y | gdk::Key::Y if ctrl => {
                if st_k.borrow_mut().redo() {
                    queue_canvas(&cv_k);
                }
                glib::Propagation::Stop
            }
            gdk::Key::o | gdk::Key::O if ctrl => {
                open_file(&win_k, &st_k, &lc_k, &cv_k, &recent_k);
                glib::Propagation::Stop
            }
            gdk::Key::s | gdk::Key::S if ctrl => {
                save_file(&win_k, &st_k, &lc_k, &cv_k);
                glib::Propagation::Stop
            }
            gdk::Key::n | gdk::Key::N if ctrl => {
                new_document_dialog(&win_k, &st_k, &lc_k, &cv_k);
                glib::Propagation::Stop
            }
            gdk::Key::x | gdk::Key::X if ctrl => {
                cut_selection(&st_k);
                queue_canvas(&cv_k);
                glib::Propagation::Stop
            }
            gdk::Key::c | gdk::Key::C if ctrl => {
                copy_selection(&st_k);
                glib::Propagation::Stop
            }
            gdk::Key::v | gdk::Key::V if ctrl => {
                try_paste_system_clipboard(&win_k, &st_k, &td_k, &cv_k, &lc_k);
                glib::Propagation::Stop
            }
            gdk::Key::plus | gdk::Key::equal if ctrl => {
                zoom_step(&st_k, &cv_k, 1.25);
                queue_canvas(&cv_k);
                glib::Propagation::Stop
            }
            gdk::Key::minus if ctrl => {
                zoom_step(&st_k, &cv_k, 1.0 / 1.25);
                queue_canvas(&cv_k);
                glib::Propagation::Stop
            }
            gdk::Key::_0 if ctrl => {
                zoom_to_fit(&st_k, &cv_k);
                queue_canvas(&cv_k);
                glib::Propagation::Stop
            }
            gdk::Key::Delete | gdk::Key::BackSpace if !ctrl => {
                if st_k.borrow().selection.is_some() {
                    erase_selection(&st_k);
                    queue_canvas(&cv_k);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            _ => {
                if !ctrl {
                    if let Some(ch) = keyval.to_unicode().map(|c| c.to_ascii_lowercase()) {
                        let st_ref = st_k.borrow();
                        let tool = st_ref.tool_keybinds.iter()
                            .find(|(_, bind)| *bind == Some(ch))
                            .map(|(t, _)| *t);
                        drop(st_ref);
                        if let Some(tool) = tool {
                            if let Some(ref dd) = *td_k.borrow() {
                                dd.set_selected(tool.dropdown_index());
                            }
                            queue_canvas(&cv_k);
                            return glib::Propagation::Stop;
                        }
                    }
                }
                glib::Propagation::Proceed
            }
        }
    });
    window.add_controller(key);

    let st_shutdown = state.clone();
    let da_shutdown = drawing_area.clone();
    let preview_shutdown = fg_bg_da.clone();
    let tick_shutdown = tick_slot.clone();
    app.connect_shutdown(move |_app| {
        if let Some(h) = tick_shutdown.borrow_mut().take() {
            h.remove();
        }
        da_shutdown.unset_draw_func();
        preview_shutdown.unset_draw_func();
        st_shutdown.borrow_mut().release_drawing_caches();
    });

    let st_close = state.clone();
    let lc_close = layers_cell.clone();
    let cv_close = canvas_cell.clone();
    window.connect_close_request(move |win| {
        if !st_close.borrow().modified {
            return glib::Propagation::Proceed;
        }
        let dlg = libadwaita::AlertDialog::new(
            Some("Save changes before closing?"),
            Some("If you don't save, your changes will be lost."),
        );
        dlg.add_responses(&[
            ("cancel", "Cancel"),
            ("discard", "Quit without saving"),
            ("save", "Save and quit"),
        ]);
        dlg.set_close_response("cancel");
        dlg.set_default_response(Some("cancel"));
        dlg.set_response_appearance("discard", libadwaita::ResponseAppearance::Destructive);
        dlg.set_response_appearance("save", libadwaita::ResponseAppearance::Suggested);

        let win_parent = win.clone();
        let win = win.clone();
        let st = st_close.clone();
        let lc = lc_close.clone();
        let cv = cv_close.clone();
        dlg.choose(Some(&win_parent), None::<&gio::Cancellable>, move |response| {
            match response.as_str() {
                "save" => {
                    if st.borrow().doc.path.is_some() {
                        if try_save_to_current_path(&st) {
                            win.close();
                        }
                    } else {
                        let win_done = win.clone();
                        save_file_as(
                            &win,
                            &st,
                            &lc,
                            &cv,
                            Some(Rc::new(move || win_done.close())),
                        );
                    }
                }
                "discard" => {
                    st.borrow_mut().modified = false;
                    win.close();
                }
                _ => {}
            }
        });
        glib::Propagation::Stop
    });

    window.present();

    schedule_launch_update_check(&window);

    let st_fit = state.clone();
    let cv_fit = canvas_cell.clone();
    glib::idle_add_local_once(move || {
        zoom_to_fit(&st_fit, &cv_fit);
        queue_canvas(&cv_fit);
    });
}

fn schedule_launch_update_check(window: &libadwaita::ApplicationWindow) {
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        let _ = window;
        return;
    }
    if let Ok(s) = std::env::var("WOOLYPAINT_SKIP_UPDATE_CHECK") {
        let t = s.trim();
        if t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes") {
            return;
        }
    }
    let w = window.clone();
    glib::timeout_add_seconds_local_once(2, move || {
        let win_send = glib::SendWeakRef::from(w.downgrade());
        std::thread::spawn(move || {
            let result = crate::updater::check_for_update();
            glib::MainContext::default().invoke(move || {
                if let Ok(Some(crate::updater::UpdateCheckResult::UpdateAvailable(info))) = result {
                    if let Some(win) = win_send.upgrade() {
                        present_update_dialog(&win, info);
                    }
                }
            });
        });
    });
}

fn run_manual_update_check(window: &libadwaita::ApplicationWindow) {
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        show_simple_alert(
            window,
            "Updates",
            "Automatic updates are only available for Linux x86_64 and Windows x86_64 release builds.",
        );
        return;
    }
    let win_send = glib::SendWeakRef::from(window.downgrade());
    std::thread::spawn(move || {
        let result = crate::updater::check_for_update();
        glib::MainContext::default().invoke(move || {
            let Some(w) = win_send.upgrade() else {
                return;
            };
            match result {
                Ok(Some(crate::updater::UpdateCheckResult::UpdateAvailable(info))) => {
                    present_update_dialog(&w, info);
                }
                Ok(Some(crate::updater::UpdateCheckResult::UpToDate { version })) => {
                    let body = format!("you are on the latest update (v{version})");
                    let dlg = libadwaita::AlertDialog::new(None, Some(&body));
                    dlg.add_response("ok", "OK");
                    dlg.set_default_response(Some("ok"));
                    dlg.set_close_response("ok");
                    dlg.connect_response(None, |d, _| {
                        d.close();
                    });
                    dlg.present(Some(&w));
                }
                Ok(None) => show_simple_alert(&w, "Up to date", "You are already running the latest release."),
                Err(e) => show_simple_alert(
                    &w,
                    "Update check failed",
                    &format!("Could not reach GitHub: {e}"),
                ),
            }
        });
    });
}

fn show_simple_alert(parent: &libadwaita::ApplicationWindow, heading: &str, body: &str) {
    let dlg = libadwaita::AlertDialog::new(Some(heading), Some(body));
    dlg.add_response("ok", "OK");
    dlg.set_default_response(Some("ok"));
    dlg.set_close_response("ok");
    dlg.connect_response(None, |d, _| {
        d.close();
    });
    dlg.present(Some(parent));
}

fn show_update_error(parent: &libadwaita::ApplicationWindow, message: &str) {
    show_simple_alert(parent, "Update failed", message);
}

fn present_update_dialog(parent: &libadwaita::ApplicationWindow, info: crate::updater::UpdateInfo) {
    let cur = crate::updater::packaged_version();
    let dlg = libadwaita::AlertDialog::new(
        Some("Update available"),
        Some(&format!(
            "Version {} is available (you are on v{})",
            info.version,
            cur
        )),
    );
    dlg.add_response("later", "Not now");
    dlg.add_response("release", "View release…");
    dlg.add_response("download", "Download and install");
    dlg.set_close_response("later");
    dlg.set_default_response(Some("download"));
    dlg.set_response_appearance("download", libadwaita::ResponseAppearance::Suggested);

    let parent_for_uri = parent.clone();
    let release_url = info.release_page_url.clone();
    let download_info = info;
    let win_send = glib::SendWeakRef::from(parent.downgrade());

    dlg.connect_response(None, move |d, response| {
        match response {
            "download" => {
                let ii = download_info.clone();
                let ws = win_send.clone();
                std::thread::spawn(move || {
                    if let Err(e) = crate::updater::download_and_apply(&ii) {
                        glib::MainContext::default().invoke(move || {
                            if let Some(w) = ws.upgrade() {
                                show_update_error(&w, &e.to_string());
                            }
                        });
                    }
                });
                d.close();
            }
            "release" => {
                let launcher = gtk::UriLauncher::new(&release_url);
                launcher.launch(Some(&parent_for_uri), None::<&gio::Cancellable>, |res| {
                    if let Err(e) = res {
                        eprintln!("Could not open release page: {e}");
                    }
                });
            }
            _ => {
                d.close();
            }
        }
    });
    dlg.present(Some(parent));
}

fn color_scheme_from_menu_value(s: &str) -> ColorScheme {
    match s {
        "light" => ColorScheme::ForceLight,
        "dark" => ColorScheme::ForceDark,
        "default" | _ => ColorScheme::Default,
    }
}

fn menu_value_for_color_scheme(scheme: ColorScheme) -> &'static str {
    match scheme {
        ColorScheme::ForceLight | ColorScheme::PreferLight => "light",
        ColorScheme::ForceDark | ColorScheme::PreferDark => "dark",
        ColorScheme::Default => "default",
        _ => "default",
    }
}

fn build_menu(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
    tool_dd_cell: &ToolDdCell,
    tool_strings: &gtk::StringList,
    recent_menu: Rc<gio::Menu>,
    palette_sidebar: PaletteSidebar,
) -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("New…"), Some("win.new"));
    file.append(Some("Open…"), Some("win.open"));
    file.append_submenu(Some("Recent Files"), &*recent_menu);
    file.append(Some("Save"), Some("win.save"));
    file.append(Some("Save As…"), Some("win.save_as"));
    menu.append_submenu(Some("_File"), &file);

    let canvas_menu = gio::Menu::new();
    canvas_menu.append(Some("Resize canvas…"), Some("win.canvas_resize"));
    canvas_menu.append(Some("Flip X"), Some("win.canvas_flip_x"));
    canvas_menu.append(Some("Flip Y"), Some("win.canvas_flip_y"));
    canvas_menu.append(Some("Rotate 90deg"), Some("win.canvas_rotate_cw"));
    canvas_menu.append(Some("Pixel grid"), Some("win.show_pixel_grid"));
    menu.append_submenu(Some("_Canvas"), &canvas_menu);

    let settings = gio::Menu::new();
    settings.append(Some("Keybinds…"), Some("win.keybinds"));
    settings.append(Some("Check for Updates…"), Some("win.check_updates"));
    let theme = gio::Menu::new();
    theme.append(Some("_Default"), Some("win.color_scheme('default')"));
    theme.append(Some("_Light"), Some("win.color_scheme('light')"));
    theme.append(Some("_Dark"), Some("win.color_scheme('dark')"));
    settings.append_submenu(Some("Color _theme"), &theme);
    let pal_menu = gio::Menu::new();
    pal_menu.append(Some("Import hex…"), Some("win.palette_import"));
    pal_menu.append(Some("Export hex…"), Some("win.palette_export"));
    pal_menu.append(Some("Manage palettes…"), Some("win.palette_manage"));
    settings.append_submenu(Some("_Palettes"), &pal_menu);
    menu.append_submenu(Some("_Settings"), &settings);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let w = window.clone();

    let new_act = gio::SimpleAction::new("new", None);
    new_act.connect_activate(move |_, _| {
        new_document_dialog(&w, &st, &lc, &cv);
    });
    app_add_action(window, &new_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let w = window.clone();
    let recent_open = recent_menu.clone();
    let open_act = gio::SimpleAction::new("open", None);
    open_act.connect_activate(move |_, _| {
        open_file(&w, &st, &lc, &cv, &recent_open);
    });
    app_add_action(window, &open_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let recent_pick = recent_menu.clone();
    let open_recent_act = gio::SimpleAction::new("open_recent", Some(glib::VariantTy::STRING));
    open_recent_act.connect_activate(move |_, param| {
        let Some(p) = param else {
            return;
        };
        let Some(s) = p.get::<String>() else {
            return;
        };
        open_document_from_path(Path::new(&s), &st, &lc, &cv, &recent_pick);
    });
    app_add_action(window, &open_recent_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let w = window.clone();
    let save_act = gio::SimpleAction::new("save", None);
    save_act.connect_activate(move |_, _| {
        save_file(&w, &st, &lc, &cv);
    });
    app_add_action(window, &save_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let w = window.clone();
    let save_as_act = gio::SimpleAction::new("save_as", None);
    save_as_act.connect_activate(move |_, _| {
        save_file_as(&w, &st, &lc, &cv, None);
    });
    app_add_action(window, &save_as_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let w = window.clone();
    let canvas_resize_act = gio::SimpleAction::new("canvas_resize", None);
    canvas_resize_act.connect_activate(move |_, _| {
        canvas_resize_dialog(&w, &st, &lc, &cv);
    });
    app_add_action(window, &canvas_resize_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let canvas_flip_x_act = gio::SimpleAction::new("canvas_flip_x", None);
    canvas_flip_x_act.connect_activate(move |_, _| {
        let mut g = st.borrow_mut();
        commit_floating(&mut g);
        g.doc.flip_x();
        finalize_canvas_geometry_change(&mut g);
        drop(g);
        refresh_after_canvas_change(&st, &lc, &cv, false);
    });
    app_add_action(window, &canvas_flip_x_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let canvas_flip_y_act = gio::SimpleAction::new("canvas_flip_y", None);
    canvas_flip_y_act.connect_activate(move |_, _| {
        let mut g = st.borrow_mut();
        commit_floating(&mut g);
        g.doc.flip_y();
        finalize_canvas_geometry_change(&mut g);
        drop(g);
        refresh_after_canvas_change(&st, &lc, &cv, false);
    });
    app_add_action(window, &canvas_flip_y_act);

    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    let canvas_rotate_act = gio::SimpleAction::new("canvas_rotate_cw", None);
    canvas_rotate_act.connect_activate(move |_, _| {
        let mut g = st.borrow_mut();
        commit_floating(&mut g);
        g.doc.rotate_90_cw();
        finalize_canvas_geometry_change(&mut g);
        drop(g);
        refresh_after_canvas_change(&st, &lc, &cv, true);
    });
    app_add_action(window, &canvas_rotate_act);

    let st_grid = state.clone();
    let cv_grid = canvas.clone();
    let initial_grid = state.borrow().show_pixel_grid;
    let pixel_grid_act = gio::SimpleAction::new_stateful(
        "show_pixel_grid",
        None,
        &initial_grid.to_variant(),
    );
    pixel_grid_act.connect_activate(move |action, _| {
        let current = action
            .state()
            .and_then(|v| v.get::<bool>())
            .unwrap_or(false);
        let new_state = !current;
        action.set_state(&new_state.to_variant());
        st_grid.borrow_mut().show_pixel_grid = new_state;
        queue_canvas(&cv_grid);
        crate::settings::persist(&st_grid.borrow());
    });
    app_add_action(window, &pixel_grid_act);

    let st = state.clone();
    let cv = canvas.clone();
    let undo_act = gio::SimpleAction::new("undo", None);
    undo_act.connect_activate(move |_, _| {
        if st.borrow_mut().undo() {
            queue_canvas(&cv);
        }
    });
    app_add_action(window, &undo_act);

    let st = state.clone();
    let cv = canvas.clone();
    let redo_act = gio::SimpleAction::new("redo", None);
    redo_act.connect_activate(move |_, _| {
        if st.borrow_mut().redo() {
            queue_canvas(&cv);
        }
    });
    app_add_action(window, &redo_act);

    let st = state.clone();
    let cv = canvas.clone();
    let cut_act = gio::SimpleAction::new("cut", None);
    cut_act.connect_activate(move |_, _| {
        cut_selection(&st);
        queue_canvas(&cv);
    });
    app_add_action(window, &cut_act);

    let st = state.clone();
    let copy_act = gio::SimpleAction::new("copy", None);
    copy_act.connect_activate(move |_, _| {
        copy_selection(&st);
    });
    app_add_action(window, &copy_act);

    let st = state.clone();
    let cv = canvas.clone();
    let td_p = tool_dd_cell.clone();
    let lc_p = layers_cell.clone();
    let w_p = window.clone();
    let paste_act = gio::SimpleAction::new("paste", None);
    paste_act.connect_activate(move |_, _| {
        try_paste_system_clipboard(&w_p, &st, &td_p, &cv, &lc_p);
    });
    app_add_action(window, &paste_act);

    let st = state.clone();
    let w_kb = window.clone();
    let ts_kb = tool_strings.clone();
    let kb_act = gio::SimpleAction::new("keybinds", None);
    kb_act.connect_activate(move |_, _| {
        keybinds_dialog(&w_kb, &st, &ts_kb);
    });
    app_add_action(window, &kb_act);

    let w_up = window.clone();
    let check_act = gio::SimpleAction::new("check_updates", None);
    check_act.connect_activate(move |_, _| {
        run_manual_update_check(&w_up);
    });
    app_add_action(window, &check_act);

    let initial_theme = menu_value_for_color_scheme(libadwaita::StyleManager::default().color_scheme());
    let theme_act = gio::SimpleAction::new_stateful(
        "color_scheme",
        Some(glib::VariantTy::STRING),
        &initial_theme.to_variant(),
    );
    let st_theme = state.clone();
    theme_act.connect_activate(move |act, param| {
        let Some(p) = param else {
            return;
        };
        let Some(s) = p.get::<String>() else {
            return;
        };
        libadwaita::StyleManager::default().set_color_scheme(color_scheme_from_menu_value(s.as_str()));
        act.set_state(p);
        crate::settings::persist(&st_theme.borrow());
    });
    app_add_action(window, &theme_act);

    let st = state.clone();
    let cv = canvas.clone();
    let zi_act = gio::SimpleAction::new("zoom_in", None);
    zi_act.connect_activate(move |_, _| {
        zoom_step(&st, &cv, 1.25);
        queue_canvas(&cv);
    });
    app_add_action(window, &zi_act);

    let st = state.clone();
    let cv = canvas.clone();
    let zo_act = gio::SimpleAction::new("zoom_out", None);
    zo_act.connect_activate(move |_, _| {
        zoom_step(&st, &cv, 1.0 / 1.25);
        queue_canvas(&cv);
    });
    app_add_action(window, &zo_act);

    let st = state.clone();
    let cv = canvas.clone();
    let zf_act = gio::SimpleAction::new("zoom_fit", None);
    zf_act.connect_activate(move |_, _| {
        zoom_to_fit(&st, &cv);
        queue_canvas(&cv);
    });
    app_add_action(window, &zf_act);

    let st = state.clone();
    let w_pi = window.clone();
    let ps_i = palette_sidebar.clone();
    let palette_import_act = gio::SimpleAction::new("palette_import", None);
    palette_import_act.connect_activate(move |_, _| {
        import_palette_file(&w_pi, &st, &ps_i);
    });
    app_add_action(window, &palette_import_act);

    let st = state.clone();
    let w_pe = window.clone();
    let palette_export_act = gio::SimpleAction::new("palette_export", None);
    palette_export_act.connect_activate(move |_, _| {
        export_palette_file(&w_pe, &st);
    });
    app_add_action(window, &palette_export_act);

    let st = state.clone();
    let w_pm = window.clone();
    let ps_m = palette_sidebar.clone();
    let palette_manage_act = gio::SimpleAction::new("palette_manage", None);
    palette_manage_act.connect_activate(move |_, _| {
        manage_palettes_dialog(&w_pm, &st, &ps_m);
    });
    app_add_action(window, &palette_manage_act);

    menu
}


fn app_add_action(window: &libadwaita::ApplicationWindow, action: &gio::SimpleAction) {
    window.add_action(action);
}
