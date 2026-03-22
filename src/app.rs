use crate::document::{
    composite_layers_into, premul_to_straight_rgba, premul_to_straight_rgba_into, straight_to_premul,
    Document,
};
use crate::state::{AppState, FloatingSelection};
use crate::tools::{
    clear_rect, copy_rect, draw_ellipse, draw_rect_outline, flood_fill, paste_rect,
    sample_composite_premul, stamp_circle, stamp_square, stroke_line, stroke_line_square, ToolKind,
};
use libadwaita::prelude::*;
use libadwaita::{Application, ColorScheme};
use gdk_pixbuf::Pixbuf;
use gtk::gdk;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
#[allow(deprecated)]
use gtk::prelude::ColorChooserExt;
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

type SharedState = Rc<RefCell<AppState>>;
type CanvasCell = Rc<RefCell<Option<gtk::DrawingArea>>>;
type LayersCell = Rc<RefCell<Option<gtk::ListBox>>>;
type ColorPreviewDaCell = Rc<RefCell<Option<gtk::DrawingArea>>>;
type ToolDdCell = Rc<RefCell<Option<gtk::DropDown>>>;

pub fn run() -> gtk::glib::ExitCode {
    libadwaita::init().expect("libadwaita init");
    libadwaita::StyleManager::default()
        .set_color_scheme(libadwaita::ColorScheme::Default);
    let app = Application::builder()
        .application_id("dev.woolymelon.WoolyPaint")
        .build();

    app.connect_activate(build_ui);
    app.run()
}

fn tool_label(tool: ToolKind, key: Option<char>) -> String {
    match key {
        Some(c) => format!("{} ({})", tool.display_name(), c.to_ascii_uppercase()),
        None => tool.display_name().to_string(),
    }
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

/// Straight RGBA presets shown in the sidebar (black/white, then rainbow, then grays, brown).
const SIDEBAR_DEFAULT_COLORS: &[[u8; 4]] = &[
    [0, 0, 0, 255],
    [255, 255, 255, 255],
    [255, 0, 0, 255],
    [255, 128, 0, 255],
    [255, 200, 0, 255],
    [0, 160, 0, 255],
    [0, 220, 220, 255],
    [0, 100, 255, 255],
    [160, 0, 255, 255],
    [255, 0, 200, 255],
    [64, 64, 64, 255],
    [128, 128, 128, 255],
    [192, 192, 192, 255],
    [139, 90, 43, 255],
];

fn fg_to_rgba(fg: [u8; 4]) -> gdk::RGBA {
    gdk::RGBA::new(
        fg[0] as f32 / 255.0,
        fg[1] as f32 / 255.0,
        fg[2] as f32 / 255.0,
        fg[3] as f32 / 255.0,
    )
}

fn rgba_to_fg(c: &gdk::RGBA) -> [u8; 4] {
    [
        (c.red() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.green() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.blue() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.alpha() * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn push_recent_color(st: &mut AppState, fg: [u8; 4]) {
    st.recent_colors.retain(|c| *c != fg);
    st.recent_colors.insert(0, fg);
    st.recent_colors.truncate(4);
}

fn swatch_button(fg: [u8; 4]) -> gtk::Button {
    let da = gtk::DrawingArea::builder()
        .width_request(24)
        .height_request(24)
        .build();
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
    let btn = gtk::Button::builder().child(&da).css_classes(["flat"]).build();
    btn.set_tooltip_text(Some(&format!("RGB {}, {}, {}", fg[0], fg[1], fg[2])));
    btn
}

fn make_color_preview_area(state: &SharedState) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::builder()
        .width_request(44)
        .height_request(44)
        .build();
    let st_draw = state.clone();
    da.set_draw_func(move |_d, cr, w, h| {
        let st = st_draw.borrow();
        let fg = st.fg;
        cr.set_source_rgba(
            fg[0] as f64 / 255.0,
            fg[1] as f64 / 255.0,
            fg[2] as f64 / 255.0,
            fg[3] as f64 / 255.0,
        );
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        let _ = cr.fill();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
        cr.set_line_width(1.0);
        cr.rectangle(0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0);
        let _ = cr.stroke();
    });
    da
}

fn refresh_recent_swatch_row(
    flow: &gtk::FlowBox,
    state: &SharedState,
    preview_da: &gtk::DrawingArea,
    cv: &CanvasCell,
) {
    while let Some(c) = flow.first_child() {
        flow.remove(&c);
    }
    let recents: Vec<[u8; 4]> = state.borrow().recent_colors.clone();
    for fg in recents {
        let btn = swatch_button(fg);
        let st = state.clone();
        let prev = preview_da.clone();
        let cv2 = cv.clone();
        let flow2 = flow.clone();
        btn.connect_clicked(move |_| {
            {
                let mut g = st.borrow_mut();
                g.fg = fg;
                push_recent_color(&mut g, fg);
            }
            prev.queue_draw();
            queue_canvas(&cv2);
            refresh_recent_swatch_row(&flow2, &st, &prev, &cv2);
        });
        flow.append(&btn);
    }
}

#[allow(deprecated)]
fn present_custom_color_dialog(
    parent: &libadwaita::ApplicationWindow,
    state: &SharedState,
    preview_da: &gtk::DrawingArea,
    recent_flow: &gtk::FlowBox,
    cv: &CanvasCell,
) {
    let win = libadwaita::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Custom color")
        .default_width(420)
        .default_height(460)
        .resizable(true)
        .build();

    let chooser = gtk::ColorChooserWidget::new();
    chooser.set_use_alpha(true);
    chooser.set_show_editor(true);
    chooser.set_rgba(&fg_to_rgba(state.borrow().fg));

    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    outer.append(&chooser);

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("OK");
    btn_row.append(&cancel);
    btn_row.append(&ok);
    outer.append(&btn_row);
    win.set_content(Some(&outer));

    let st_ok = state.clone();
    let prev_ok = preview_da.clone();
    let rf_ok = recent_flow.clone();
    let cv_ok = cv.clone();
    let w_ok = win.clone();
    let ch_ok = chooser.clone();
    ok.connect_clicked(move |_| {
        let c = ch_ok.rgba();
        let fg = rgba_to_fg(&c);
        {
            let mut g = st_ok.borrow_mut();
            g.fg = fg;
            push_recent_color(&mut g, fg);
        }
        prev_ok.queue_draw();
        queue_canvas(&cv_ok);
        refresh_recent_swatch_row(&rf_ok, &st_ok, &prev_ok, &cv_ok);
        w_ok.close();
    });

    let w_cancel = win.clone();
    cancel.connect_clicked(move |_| w_cancel.close());

    win.present();
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
            *slot = Some(p);
        }
        f(slot.as_ref().unwrap());
    });
}

fn draw_canvas(state: &SharedState, cr: &gtk::cairo::Context) {
    let (w, h, pan_x, pan_y, zoom, floating, selection) = {
        let st = state.borrow();
        (
            st.doc.width,
            st.doc.height,
            st.pan_x,
            st.pan_y,
            st.zoom,
            st.floating.clone(),
            st.selection,
        )
    };
    let len = (w * h * 4) as usize;
    let stride = (w * 4) as i32;

    let bytes = {
        let mut st = state.borrow_mut();
        let use_cache = !st.brush_stroke_in_progress
            && st.composite_cache_at_revision == st.document_visual_revision
            && st.composite_cache_straight.len() == len;
        if !use_cache {
            let AppState {
                ref doc,
                ref mut composite_cache_premul,
                ref mut composite_cache_straight,
                ref mut composite_cache_at_revision,
                document_visual_revision,
                ..
            } = &mut *st;
            composite_cache_premul.resize(len, 0);
            composite_layers_into(composite_cache_premul, doc.width, doc.height, &doc.layers);
            composite_cache_straight.resize(len, 0);
            premul_to_straight_rgba_into(composite_cache_straight, composite_cache_premul);
            *composite_cache_at_revision = *document_visual_revision;
        }
        glib::Bytes::from_owned(st.composite_cache_straight.clone())
    };

    let pixbuf = Pixbuf::from_bytes(
        &bytes,
        gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        w as i32,
        h as i32,
        stride,
    );

    cr.save().unwrap();
    cr.translate(pan_x, pan_y);
    cr.scale(zoom, zoom);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    cr.clip();
    with_transparency_checker_pattern(|pat| {
        cr.set_source(pat).unwrap();
        cr.paint().unwrap();
    });
    cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
    cr.source().set_filter(gtk::cairo::Filter::Nearest);
    cr.paint().unwrap();
    cr.restore().unwrap();

    if let Some(ref f) = floating {
        let fs = premul_to_straight_rgba(&f.data);
        let fw = f.w.max(1);
        let fh = f.h.max(1);
        let pb = Pixbuf::from_bytes(
            &glib::Bytes::from_owned(fs),
            gdk_pixbuf::Colorspace::Rgb,
            true,
            8,
            fw,
            fh,
            fw * 4,
        );
        cr.save().unwrap();
        cr.translate(pan_x, pan_y);
        cr.scale(zoom, zoom);
        cr.translate(f.x, f.y);
        cr.set_source_pixbuf(&pb, 0.0, 0.0);
        cr.source().set_filter(gtk::cairo::Filter::Nearest);
        cr.paint().unwrap();
        cr.restore().unwrap();
    }

    if let Some((sx, sy, sw, sh)) = selection {
        cr.save().unwrap();
        cr.translate(pan_x, pan_y);
        cr.scale(zoom, zoom);
        let phase = (glib::monotonic_time() / 50000) % 20;
        cr.set_dash(&[6.0, 6.0], phase as f64);
        cr.set_line_width(1.0 / zoom.max(0.001));
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.rectangle(sx as f64, sy as f64, sw as f64, sh as f64);
        cr.stroke().unwrap();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.95);
        cr.set_dash(&[6.0, 6.0], (phase as f64) + 6.0);
        cr.rectangle(sx as f64, sy as f64, sw as f64, sh as f64);
        cr.stroke().unwrap();
        cr.restore().unwrap();
    }
}

fn point_in_sel(x: f64, y: f64, sel: (i32, i32, i32, i32)) -> bool {
    let (sx, sy, sw, sh) = sel;
    x >= sx as f64
        && y >= sy as f64
        && x < (sx + sw) as f64
        && y < (sy + sh) as f64
}

fn commit_floating(state: &mut AppState) {
    let Some(f) = state.floating.take() else {
        return;
    };
    let idx = state.doc.active_layer;
    let Some(layer) = state.doc.layers.get_mut(idx) else {
        return;
    };
    let before = layer.pixels.clone();
    paste_rect(
        layer,
        f.x.round() as i32,
        f.y.round() as i32,
        f.w,
        f.h,
        &f.data,
    );
    if layer.pixels != before {
        state.history.commit_change(idx, before);
        state.modified = true;
        state.bump_document_revision();
    }
}

fn setup_canvas_input(
    canvas: &gtk::DrawingArea,
    state: &SharedState,
    canvas_cell: &CanvasCell,
    color_preview_da_cell: &ColorPreviewDaCell,
    recent_swatches: &gtk::FlowBox,
) {
    let brush_widget_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));
    let move_widget_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));

    let drag = gtk::GestureDrag::new();
    let st_drag_begin = state.clone();
    let cv_drag = canvas_cell.clone();
    let cnv = canvas.clone();
    let bws = brush_widget_start.clone();
    let mws_b = move_widget_start.clone();
    let cb_drag = color_preview_da_cell.clone();
    let recent_drag = recent_swatches.clone();
    drag.connect_drag_begin(move |_g, wx, wy| {
        cnv.grab_focus();
        let mut st = st_drag_begin.borrow_mut();
        let (dx, dy) = st.widget_to_doc(wx, wy);
        let mut eyedrop_updated = false;
        match st.tool {
            ToolKind::Brush | ToolKind::Eraser => {
                let color = st.fg;
                let eraser = st.tool == ToolKind::Eraser;
                let radius = st.brush_size * 0.5;
                let hardness = st.brush_hardness;
                st.brush_stroke_in_progress = true;
                st.begin_stroke_undo();
                st.last_doc_pos = Some((dx, dy));
                *bws.borrow_mut() = Some((wx, wy));
                let layer = match st.doc.active_layer_mut() {
                    Some(l) => l,
                    None => return,
                };
                stamp_circle(layer, dx, dy, radius, hardness, color, eraser);
                st.modified = true;
            }
            ToolKind::Pixel => {
                let color = st.fg;
                let size = st.brush_size;
                st.brush_stroke_in_progress = true;
                st.begin_stroke_undo();
                st.last_doc_pos = Some((dx, dy));
                *bws.borrow_mut() = Some((wx, wy));
                let layer = match st.doc.active_layer_mut() {
                    Some(l) => l,
                    None => return,
                };
                stamp_square(layer, dx, dy, size, color, false);
                st.modified = true;
            }
            ToolKind::Fill => {
                let fg = st.fg;
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
                let premul: Cow<'_, [u8]> = if !st.brush_stroke_in_progress
                    && st.composite_cache_at_revision == st.document_visual_revision
                    && st.composite_cache_premul.len() == clen
                {
                    Cow::Borrowed(st.composite_cache_premul.as_slice())
                } else {
                    Cow::Owned(st.doc.composite())
                };
                let c = sample_composite_premul(
                    premul.as_ref(),
                    cw,
                    ch,
                    dx.floor() as i32,
                    dy.floor() as i32,
                );
                st.fg = c;
                push_recent_color(&mut st, c);
                eyedrop_updated = true;
            }
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse | ToolKind::SelectRect => {
                st.drag_start_doc = Some((dx, dy));
            }
            ToolKind::Hand => {
                *bws.borrow_mut() = Some((st.pan_x, st.pan_y));
            }
            ToolKind::Move => {
                if let Some(ref f) = st.floating {
                    if dx < f.x || dy < f.y || dx >= f.x + f.w as f64 || dy >= f.y + f.h as f64 {
                        commit_floating(&mut st);
                    }
                }
                if st.floating.is_none() {
                    if let Some(sel) = st.selection {
                        if point_in_sel(dx, dy, sel) {
                            let (sx, sy, sw, sh) = sel;
                            let layer = match st.doc.active_layer_mut() {
                                Some(l) => l,
                                None => return,
                            };
                            let before = layer.pixels.clone();
                            let data = copy_rect(layer, sx, sy, sw, sh);
                            clear_rect(layer, sx, sy, sw, sh);
                            let li = st.doc.active_layer;
                            st.history.commit_change(li, before);
                            st.bump_document_revision();
                            st.floating = Some(FloatingSelection {
                                w: sw,
                                h: sh,
                                data,
                                x: sx as f64,
                                y: sy as f64,
                            });
                            st.selection = None;
                            st.modified = true;
                        }
                    }
                }
                if let Some(ref f) = st.floating {
                    st.move_grab_doc = Some((dx - f.x, dy - f.y));
                    *mws_b.borrow_mut() = Some((wx, wy));
                }
            }
        }
        drop(st);
        if eyedrop_updated {
            if let Some(ref da) = *cb_drag.borrow() {
                da.queue_draw();
                refresh_recent_swatch_row(&recent_drag, &st_drag_begin, da, &cv_drag);
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
                let (cx, cy) = st.widget_to_doc(cur_wx, cur_wy);
                let Some((lx, ly)) = st.last_doc_pos else {
                    st.last_doc_pos = Some((cx, cy));
                    return;
                };
                let color = st.fg;
                let eraser = st.tool == ToolKind::Eraser;
                let radius = st.brush_size * 0.5;
                let hardness = st.brush_hardness;
                let layer = match st.doc.active_layer_mut() {
                    Some(l) => l,
                    None => return,
                };
                stroke_line(layer, lx, ly, cx, cy, radius, hardness, color, eraser);
                st.last_doc_pos = Some((cx, cy));
                st.modified = true;
            }
            ToolKind::Pixel => {
                let Some((bx, by)) = *bws_up.borrow() else {
                    return;
                };
                let (cx, cy) = st.widget_to_doc(bx + ox, by + oy);
                let Some((lx, ly)) = st.last_doc_pos else {
                    st.last_doc_pos = Some((cx, cy));
                    return;
                };
                let color = st.fg;
                let size = st.brush_size;
                let layer = match st.doc.active_layer_mut() {
                    Some(l) => l,
                    None => return,
                };
                stroke_line_square(layer, lx, ly, cx, cy, size, color, false);
                st.last_doc_pos = Some((cx, cy));
                st.modified = true;
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
                st.selection = Some(AppState::normalize_rect(x0, y0, cx, cy));
            }
            ToolKind::Hand => {
                if let Some((px0, py0)) = *bws_up.borrow() {
                    st.pan_x = px0 + ox;
                    st.pan_y = py0 + oy;
                }
            }
            ToolKind::Move => {
                let grab = st.move_grab_doc;
                let wpress = *mws_u.borrow();
                let px = st.pan_x;
                let py = st.pan_y;
                let z = st.zoom;
                if let (Some(ref mut f), Some(g), Some((wpx, wpy))) =
                    (&mut st.floating, grab, wpress)
                {
                    let (cx, cy) = ((wpx + ox - px) / z, (wpy + oy - py) / z);
                    f.x = cx - g.0;
                    f.y = cy - g.1;
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
    let mws_e = move_widget_start.clone();
    drag.connect_drag_end(move |_g, ox, oy| {
        let mut st = st_drag_end.borrow_mut();
        match st.tool {
            ToolKind::Brush | ToolKind::Eraser | ToolKind::Pixel => {
                *bws_end.borrow_mut() = None;
                st.commit_stroke_undo();
                st.brush_stroke_in_progress = false;
                st.last_doc_pos = None;
            }
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => {
                if let Some((sx, sy)) = st.drag_start_doc {
                    let start_wx = sx * st.zoom + st.pan_x;
                    let start_wy = sy * st.zoom + st.pan_y;
                    let cur_wx = start_wx + ox;
                    let cur_wy = start_wy + oy;
                    let (cx, cy) = st.widget_to_doc(cur_wx, cur_wy);
                    let tool = st.tool;
                    let color = st.fg;
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
                    st.selection = Some(AppState::normalize_rect(sx, sy, cx, cy));
                }
                st.drag_start_doc = None;
            }
            ToolKind::Hand => {
                *bws_end.borrow_mut() = None;
            }
            ToolKind::Move => {
                st.move_grab_doc = None;
                st.drag_start_doc = None;
                *mws_e.borrow_mut() = None;
            }
            _ => {}
        }
        drop(st);
        queue_canvas(&cv_drag_end);
        cnv3.queue_draw();
    });

    canvas.add_controller(drag);

    let motion = gtk::EventControllerMotion::new();
    let st_m = state.clone();
    let cv_m = canvas_cell.clone();
    let last = Rc::new(RefCell::new(None::<(f64, f64)>));
    let last_c = last.clone();
    motion.connect_motion(move |ec, x, y| {
        if !ec
            .current_event_state()
            .contains(gdk::ModifierType::BUTTON2_MASK)
        {
            *last_c.borrow_mut() = Some((x, y));
            return;
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
    });
    canvas.add_controller(motion);

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
    let Some((sx, sy, sw, sh)) = st.selection else {
        return;
    };
    let idx = st.doc.active_layer;
    let Some(layer) = st.doc.layers.get_mut(idx) else {
        return;
    };
    let before = layer.pixels.clone();
    let data = copy_rect(layer, sx, sy, sw, sh);
    clear_rect(layer, sx, sy, sw, sh);
    st.clipboard = Some((sw, sh, data));
    st.history.commit_change(idx, before);
    st.selection = None;
    st.modified = true;
    st.bump_document_revision();
}

fn copy_selection(state: &SharedState) {
    let mut st = state.borrow_mut();
    let Some((sx, sy, sw, sh)) = st.selection else {
        return;
    };
    let Some(layer) = st.doc.active_layer_ref() else {
        return;
    };
    let data = copy_rect(layer, sx, sy, sw, sh);
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
    st.floating = Some(FloatingSelection { w: sw, h: sh, data, x, y });
    st.selection = None;
    st.tool = ToolKind::Move;
    drop(st);
    if let Some(ref dd) = *tool_dd_cell.borrow() {
        dd.set_selected(9);
    }
}

fn paste_image_data(state: &SharedState, tool_dd_cell: &ToolDdCell, w: u32, h: u32, premul_data: Vec<u8>) {
    let mut st = state.borrow_mut();
    commit_floating(&mut st);
    let doc_w = st.doc.width as i32;
    let doc_h = st.doc.height as i32;
    let x = ((doc_w - w as i32) / 2).max(0) as f64;
    let y = ((doc_h - h as i32) / 2).max(0) as f64;
    st.floating = Some(FloatingSelection { w: w as i32, h: h as i32, data: premul_data, x, y });
    st.selection = None;
    st.tool = ToolKind::Move;
    drop(st);
    if let Some(ref dd) = *tool_dd_cell.borrow() {
        dd.set_selected(9);
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
                                show_paste_oversize_dialog(&win, &st, &td, &cv, &lc, iw, ih, premul);
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

fn open_file(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
) {
    let all_filter = gtk::FileFilter::new();
    all_filter.set_name(Some("All supported (*.png, *.ora)"));
    all_filter.add_pattern("*.png");
    all_filter.add_pattern("*.ora");
    let png_filter = gtk::FileFilter::new();
    png_filter.set_name(Some("PNG image (*.png)"));
    png_filter.add_pattern("*.png");
    let ora_filter = gtk::FileFilter::new();
    ora_filter.set_name(Some("OpenRaster (*.ora)"));
    ora_filter.add_pattern("*.ora");

    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&all_filter);
    filters.append(&png_filter);
    filters.append(&ora_filter);

    let dlg = gtk::FileDialog::builder()
        .title("Open image")
        .modal(true)
        .filters(&filters)
        .default_filter(&all_filter)
        .build();
    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    dlg.open(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            if let Some(path) = file.path() {
                let result = match path.extension().and_then(|e| e.to_str()) {
                    Some("ora") => Document::load_ora(&path),
                    _ => Document::load_png(&path),
                };
                match result {
                    Ok(doc) => {
                        let mut g = st.borrow_mut();
                        g.doc = doc;
                        g.history.clear();
                        g.doc.path = Some(path);
                        g.modified = false;
                        g.selection = None;
                        g.floating = None;
                        g.bump_document_revision();
                        drop(g);
                        zoom_to_fit(&st, &cv);
                        refresh_layers_list(&st, &lc, &cv);
                        queue_canvas(&cv);
                    }
                    Err(e) => {
                        eprintln!("Open failed: {e}");
                    }
                }
            }
        }
    });
}

fn save_file_as(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
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

    let dlg = gtk::FileDialog::builder()
        .title("Save image")
        .modal(true)
        .filters(&filters)
        .default_filter(&png_filter)
        .build();
    let st = state.clone();
    let cv = canvas.clone();
    let _lc = layers_cell.clone();
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
    let path_opt = state.borrow().doc.path.clone();
    if let Some(path) = path_opt {
        let mut g = state.borrow_mut();
        let result = match path.extension().and_then(|e| e.to_str()) {
            Some("ora") => g.doc.save_ora(&path),
            _ => g.doc.save_png(&path),
        };
        if let Err(e) = result {
            eprintln!("Save failed: {e}");
        } else {
            g.modified = false;
        }
    } else {
        save_file_as(window, state, layers_cell, canvas);
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
        if !ch.is_ascii_alphanumeric() {
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

    let state: SharedState = Rc::new(RefCell::new(AppState::new()));
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

    let st_tick = state.clone();
    let cv_tick = canvas_cell.clone();
    let last_march = Rc::new(RefCell::new(0i64));
    let lm = last_march.clone();
    drawing_area.add_tick_callback(move |_w, _clock| {
        let st = st_tick.borrow();
        if st.selection.is_some() {
            let now = glib::monotonic_time();
            let mut last = lm.borrow_mut();
            if now.saturating_sub(*last) >= 50_000 {
                *last = now;
                queue_canvas(&cv_tick);
            }
        }
        glib::ControlFlow::Continue
    });

    let recent_colors_flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .max_children_per_line(2)
        .row_spacing(4)
        .column_spacing(4)
        .hexpand(false)
        .halign(gtk::Align::Start)
        .build();

    let preview_da = make_color_preview_area(&state);
    *color_preview_da_cell.borrow_mut() = Some(preview_da.clone());

    setup_canvas_input(
        &drawing_area,
        &state,
        &canvas_cell,
        &color_preview_da_cell,
        &recent_colors_flow,
    );

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
        st_sel.borrow_mut().doc.active_layer = idx;
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
    tool_dd.set_width_request(160);

    let size_adj_cell: Rc<RefCell<Option<gtk::Adjustment>>> = Rc::new(RefCell::new(None));
    let st_dd = state.clone();
    let cv_dd = canvas_cell.clone();
    let sa_dd = size_adj_cell.clone();
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
            9 => ToolKind::Move,
            10 => ToolKind::Hand,
            _ => ToolKind::Brush,
        };
        let cur = g.tool;
        drop(g);
        if let Some(ref adj) = *sa_dd.borrow() {
            if cur == ToolKind::Pixel && prev != ToolKind::Pixel {
                adj.set_value(1.0);
            } else if prev == ToolKind::Pixel && cur != ToolKind::Pixel {
                adj.set_value(8.0);
            }
        }
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

    let hard_adj = gtk::Adjustment::new(0.85, 0.0, 1.0, 0.01, 0.1, 0.0);
    let hard_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&hard_adj));
    hard_scale.set_hexpand(true);
    hard_scale.set_width_request(120);
    let st_h = state.clone();
    hard_adj.connect_value_changed(move |a| {
        st_h.borrow_mut().brush_hardness = a.value();
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

    let toolbar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .hexpand(false)
        .build();

    let make_row = |label: &str, widget: &gtk::Widget| -> gtk::Box {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .hexpand(false)
            .build();
        let lbl = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .width_request(36)
            .build();
        lbl.add_css_class("dim-label");
        row.append(&lbl);
        row.append(widget);
        row
    };
    tool_dd.set_width_request(100);
    hard_scale.set_width_request(100);
    size_spin.set_width_request(80);
    tol_spin.set_width_request(80);

    toolbar.append(&make_row("Tool", tool_dd.upcast_ref()));
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    toolbar.append(&make_row("Size", size_spin.upcast_ref()));
    toolbar.append(&make_row("Hard", hard_scale.upcast_ref()));
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    toolbar.append(&make_row("Fill tol", tol_spin.upcast_ref()));
    toolbar.append(&fill_check);
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let zoom_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .hexpand(false)
        .build();
    let zoom_out_btn = gtk::Button::from_icon_name("zoom-out-symbolic");
    let zoom_fit_btn = gtk::Button::from_icon_name("zoom-fit-best-symbolic");
    let zoom_in_btn = gtk::Button::from_icon_name("zoom-in-symbolic");
    zoom_out_btn.set_tooltip_text(Some("Zoom out (Ctrl+−)"));
    zoom_fit_btn.set_tooltip_text(Some("Zoom to fit (Ctrl+0)"));
    zoom_in_btn.set_tooltip_text(Some("Zoom in (Ctrl++)"));
    zoom_row.append(&zoom_out_btn);
    zoom_row.append(&zoom_fit_btn);
    zoom_row.append(&zoom_in_btn);

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

    toolbar.append(&zoom_row);

    let toolbar_spacer = gtk::Box::builder()
        .vexpand(true)
        .build();
    toolbar.append(&toolbar_spacer);

    let palette_label = gtk::Label::builder()
        .label("Default colors")
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .hexpand(false)
        .build();
    palette_label.add_css_class("dim-label");
    toolbar.append(&palette_label);

    let palette_flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .max_children_per_line(2)
        .row_spacing(4)
        .column_spacing(4)
        .hexpand(false)
        .halign(gtk::Align::Start)
        .build();
    for &fg in SIDEBAR_DEFAULT_COLORS {
        let btn = swatch_button(fg);
        let st_pal = state.clone();
        let prev_pal = preview_da.clone();
        let cv_pal = canvas_cell.clone();
        let rf_pal = recent_colors_flow.clone();
        btn.connect_clicked(move |_| {
            {
                let mut g = st_pal.borrow_mut();
                g.fg = fg;
                push_recent_color(&mut g, fg);
            }
            prev_pal.queue_draw();
            queue_canvas(&cv_pal);
            refresh_recent_swatch_row(&rf_pal, &st_pal, &prev_pal, &cv_pal);
        });
        palette_flow.append(&btn);
    }
    toolbar.append(&palette_flow);

    let recent_label = gtk::Label::builder()
        .label("Last used")
        .xalign(0.0)
        .margin_top(6)
        .halign(gtk::Align::Start)
        .hexpand(false)
        .build();
    recent_label.add_css_class("dim-label");
    toolbar.append(&recent_label);
    toolbar.append(&recent_colors_flow);

    let current_label = gtk::Label::builder()
        .label("Custom color")
        .xalign(0.0)
        .margin_top(6)
        .halign(gtk::Align::Start)
        .hexpand(false)
        .build();
    current_label.add_css_class("dim-label");
    toolbar.append(&current_label);

    let preview_btn = gtk::Button::builder()
        .child(&preview_da)
        .halign(gtk::Align::Start)
        .build();
    preview_btn.set_tooltip_text(Some("Open color editor…"));
    let w_col = window.clone();
    let st_col = state.clone();
    let prev_col = preview_da.clone();
    let rf_col = recent_colors_flow.clone();
    let cv_col = canvas_cell.clone();
    preview_btn.connect_clicked(move |_| {
        present_custom_color_dialog(&w_col, &st_col, &prev_col, &rf_col, &cv_col);
    });
    toolbar.append(&preview_btn);

    refresh_recent_swatch_row(
        &recent_colors_flow,
        &state,
        &preview_da,
        &canvas_cell,
    );

    let editor = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .vexpand(true)
        .hexpand(true)
        .build();
    editor.append(&toolbar);
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
    );
    let menubar = gtk::PopoverMenuBar::from_model(Some(&menu_model));

    let header = libadwaita::HeaderBar::new();
    header.pack_start(&menubar);
    header.pack_end(&toggle_layers);
    header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let toolbar_view = libadwaita::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&split));
    window.set_content(Some(&toolbar_view));

    let key = gtk::EventControllerKey::new();
    let st_k = state.clone();
    let lc_k = layers_cell.clone();
    let cv_k = canvas_cell.clone();
    let win_k = window.clone();
    let td_k = tool_dd_cell.clone();
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
                open_file(&win_k, &st_k, &lc_k, &cv_k);
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
            _ => {
                if !ctrl && !shift {
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

    window.present();

    let st_fit = state.clone();
    let cv_fit = canvas_cell.clone();
    glib::idle_add_local_once(move || {
        zoom_to_fit(&st_fit, &cv_fit);
        queue_canvas(&cv_fit);
    });
}

fn color_scheme_from_menu_value(s: &str) -> ColorScheme {
    match s {
        "light" => ColorScheme::ForceLight,
        "dark" => ColorScheme::ForceDark,
        _ => ColorScheme::Default,
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
) -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("New…"), Some("win.new"));
    file.append(Some("Open…"), Some("win.open"));
    file.append(Some("Save"), Some("win.save"));
    file.append(Some("Save As…"), Some("win.save_as"));
    file.append(Some("Quit"), Some("win.quit"));
    menu.append_submenu(Some("_File"), &file);

    let settings = gio::Menu::new();
    settings.append(Some("Keybinds…"), Some("win.keybinds"));
    let theme = gio::Menu::new();
    theme.append(Some("_Default"), Some("win.color_scheme('default')"));
    theme.append(Some("_Light"), Some("win.color_scheme('light')"));
    theme.append(Some("_Dark"), Some("win.color_scheme('dark')"));
    settings.append_submenu(Some("Color _theme"), &theme);
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
    let open_act = gio::SimpleAction::new("open", None);
    open_act.connect_activate(move |_, _| {
        open_file(&w, &st, &lc, &cv);
    });
    app_add_action(window, &open_act);

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
        save_file_as(&w, &st, &lc, &cv);
    });
    app_add_action(window, &save_as_act);

    let win_q = window.clone();
    let quit_act = gio::SimpleAction::new("quit", None);
    quit_act.connect_activate(move |_, _| {
        win_q.application().unwrap().quit();
    });
    app_add_action(window, &quit_act);

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

    let initial_theme = menu_value_for_color_scheme(libadwaita::StyleManager::default().color_scheme());
    let theme_act = gio::SimpleAction::new_stateful(
        "color_scheme",
        Some(glib::VariantTy::STRING),
        &initial_theme.to_variant(),
    );
    theme_act.connect_activate(move |act, param| {
        let Some(p) = param else {
            return;
        };
        let Some(s) = p.get::<String>() else {
            return;
        };
        libadwaita::StyleManager::default().set_color_scheme(color_scheme_from_menu_value(s.as_str()));
        act.set_state(p);
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

    menu
}


fn app_add_action(window: &libadwaita::ApplicationWindow, action: &gio::SimpleAction) {
    window.add_action(action);
}
