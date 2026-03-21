use crate::document::{
    adjust_brightness_contrast_straight, premul_to_straight_rgba, straight_to_premul, BlendMode,
    Document,
};
use crate::state::{AppState, FloatingSelection};
use crate::tools::{
    clear_rect, copy_rect, draw_ellipse, draw_rect_outline, flood_fill, paste_rect,
    sample_composite_premul, stamp_circle, stamp_square, stroke_line, stroke_line_square, ToolKind,
};
use libadwaita::prelude::*;
use libadwaita::Application;
use gdk_pixbuf::Pixbuf;
use gtk::gdk;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

type SharedState = Rc<RefCell<AppState>>;
type CanvasCell = Rc<RefCell<Option<gtk::DrawingArea>>>;
type LayersCell = Rc<RefCell<Option<gtk::ListBox>>>;
type ColorBtnCell = Rc<RefCell<Option<gtk::ColorDialogButton>>>;
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

fn queue_canvas(canvas: &CanvasCell) {
    if let Some(ref c) = *canvas.borrow() {
        c.queue_draw();
    }
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
            .map(|(i, l)| {
                (
                    i,
                    l.name.clone(),
                    l.visible,
                    l.opacity,
                    l.blend,
                )
            })
            .collect()
    };
    let active_layer = state.borrow().doc.active_layer;

    for (i, name, visible, opacity, blend) in layers_info {
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
        let op_spin = gtk::SpinButton::new(Some(&op_adj), 1.0, 0);
        op_spin.set_width_request(56);
        let blend_strings = gtk::StringList::new(&["Normal", "Multiply", "Add"]);
        let blend_dd = gtk::DropDown::new(Some(blend_strings), gtk::Expression::NONE);
        blend_dd.set_selected(match blend {
            BlendMode::Normal => 0,
            BlendMode::Multiply => 1,
            BlendMode::Add => 2,
        });
        blend_dd.set_width_request(90);
        let up = gtk::Button::from_icon_name("go-up-symbolic");
        let down = gtk::Button::from_icon_name("go-down-symbolic");
        let del = gtk::Button::from_icon_name("edit-delete-symbolic");
        bot_row.append(&op_spin);
        bot_row.append(&blend_dd);
        bot_row.append(&up);
        bot_row.append(&down);
        bot_row.append(&del);

        outer.append(&top_row);
        outer.append(&bot_row);

        let st = state.clone();
        let cv = canvas.clone();
        vis.connect_active_notify(move |sw| {
            let mut g = st.borrow_mut();
            if let Some(l) = g.doc.layers.get_mut(i) {
                l.visible = sw.is_active();
                g.modified = true;
            }
            queue_canvas(&cv);
        });

        let st = state.clone();
        let cv = canvas.clone();
        op_adj.connect_value_changed(move |a| {
            let mut g = st.borrow_mut();
            if let Some(l) = g.doc.layers.get_mut(i) {
                l.opacity = (a.value() / 100.0) as f32;
                g.modified = true;
            }
            queue_canvas(&cv);
        });

        let st = state.clone();
        let cv = canvas.clone();
        blend_dd.connect_selected_item_notify(move |dd| {
            let mut g = st.borrow_mut();
            if let Some(l) = g.doc.layers.get_mut(i) {
                l.blend = match dd.selected() {
                    1 => BlendMode::Multiply,
                    2 => BlendMode::Add,
                    _ => BlendMode::Normal,
                };
                g.modified = true;
            }
            queue_canvas(&cv);
        });

        let st = state.clone();
        let lc2 = layers_cell.clone();
        let cv2 = canvas.clone();
        let idx = i;
        up.connect_clicked(move |_| {
            if idx > 0 {
                st.borrow_mut().doc.move_layer(idx, idx - 1);
                refresh_layers_list(&st, &lc2, &cv2);
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
                drop(g);
                refresh_layers_list(&st, &lc3, &cv3);
            }
        });

        let st = state.clone();
        let lc4 = layers_cell.clone();
        let cv4 = canvas.clone();
        let idx_del = i;
        del.connect_clicked(move |_| {
            let mut g = st.borrow_mut();
            if g.doc.remove_layer(idx_del) {
                g.history.clear();
                drop(g);
                refresh_layers_list(&st, &lc4, &cv4);
                queue_canvas(&cv4);
            }
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

fn draw_canvas(state: &SharedState, cr: &gtk::cairo::Context) {
    let st = state.borrow();
    let w = st.doc.width;
    let h = st.doc.height;
    let comp = st.doc.composite();
    let straight = premul_to_straight_rgba(&comp);
    let stride = (w * 4) as i32;
    let bytes = glib::Bytes::from_owned(straight);
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
    cr.translate(st.pan_x, st.pan_y);
    cr.scale(st.zoom, st.zoom);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    cr.clip();
    let cs = 8.0_f64;
    cr.set_source_rgb(0.93, 0.93, 0.93);
    cr.paint().unwrap();
    cr.set_source_rgb(0.78, 0.78, 0.78);
    for ry in 0..(h as f64 / cs).ceil() as i32 {
        for rx in 0..(w as f64 / cs).ceil() as i32 {
            if (rx + ry) % 2 == 0 {
                continue;
            }
            cr.rectangle(rx as f64 * cs, ry as f64 * cs, cs, cs);
        }
    }
    cr.fill().unwrap();
    cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
    cr.paint().unwrap();
    cr.restore().unwrap();

    if let Some(ref f) = st.floating {
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
        cr.translate(st.pan_x, st.pan_y);
        cr.scale(st.zoom, st.zoom);
        cr.translate(f.x, f.y);
        cr.set_source_pixbuf(&pb, 0.0, 0.0);
        cr.paint().unwrap();
        cr.restore().unwrap();
    }

    if let Some((sx, sy, sw, sh)) = st.selection {
        cr.save().unwrap();
        cr.translate(st.pan_x, st.pan_y);
        cr.scale(st.zoom, st.zoom);
        let phase = (glib::monotonic_time() / 50000) % 20;
        cr.set_dash(&[6.0, 6.0], phase as f64);
        cr.set_line_width(1.0 / st.zoom.max(0.001));
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
    }
}

fn setup_canvas_input(canvas: &gtk::DrawingArea, state: &SharedState, canvas_cell: &CanvasCell, color_btn_cell: &ColorBtnCell) {
    let brush_widget_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));
    let move_widget_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));

    let drag = gtk::GestureDrag::new();
    let st_drag_begin = state.clone();
    let cv_drag = canvas_cell.clone();
    let cnv = canvas.clone();
    let bws = brush_widget_start.clone();
    let mws_b = move_widget_start.clone();
    let cb_drag = color_btn_cell.clone();
    drag.connect_drag_begin(move |_g, wx, wy| {
        let mut st = st_drag_begin.borrow_mut();
        let (dx, dy) = st.widget_to_doc(wx, wy);
        let mut eyedropped = None;
        match st.tool {
            ToolKind::Brush | ToolKind::Eraser => {
                let color = st.fg;
                let eraser = st.tool == ToolKind::Eraser;
                let radius = st.brush_size * 0.5;
                let hardness = st.brush_hardness;
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
                let comp = st.doc.composite();
                let c = sample_composite_premul(
                    &comp,
                    st.doc.width,
                    st.doc.height,
                    dx.floor() as i32,
                    dy.floor() as i32,
                );
                st.fg = c;
                eyedropped = Some(gdk::RGBA::new(
                    c[0] as f32 / 255.0,
                    c[1] as f32 / 255.0,
                    c[2] as f32 / 255.0,
                    c[3] as f32 / 255.0,
                ));
            }
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse | ToolKind::SelectRect => {
                st.drag_start_doc = Some((dx, dy));
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
        if let Some(rgba) = eyedropped {
            if let Some(ref btn) = *cb_drag.borrow() {
                btn.set_rgba(&rgba);
            }
        }
        queue_canvas(&cv_drag);
        cnv.queue_draw();
    });

    let st_drag_up = state.clone();
    let cv_drag_up = canvas_cell.clone();
    let cnv2 = canvas.clone();
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
        queue_canvas(&cv_drag_up);
        cnv2.queue_draw();
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
    scroll.connect_scroll(move |ec, _dx, dy| {
        let Some((x, y)) = ec.current_event().and_then(|e| e.position()) else {
            return glib::Propagation::Proceed;
        };
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
        glib::Propagation::Proceed
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
    st.clipboard = Some((sw, sh, data));
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

fn apply_brightness_contrast(state: &SharedState, brightness: f32, contrast: f32) {
    let mut st = state.borrow_mut();
    let idx = st.doc.active_layer;
    let Some(layer) = st.doc.layers.get_mut(idx) else {
        return;
    };
    let before = layer.pixels.clone();
    let straight = premul_to_straight_rgba(&layer.pixels);
    let mut buf = straight;
    adjust_brightness_contrast_straight(&mut buf, brightness, contrast);
    layer.pixels = straight_to_premul(&buf);
    st.history.commit_change(idx, before);
    st.modified = true;
}

fn open_file(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
) {
    let dlg = gtk::FileDialog::new();
    let st = state.clone();
    let lc = layers_cell.clone();
    let cv = canvas.clone();
    dlg.open(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            if let Some(path) = file.path() {
                match Document::load_png(&path) {
                    Ok(doc) => {
                        let mut g = st.borrow_mut();
                        g.doc = doc;
                        g.history.clear();
                        g.doc.path = Some(path);
                        g.modified = false;
                        g.selection = None;
                        g.floating = None;
                        drop(g);
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
    let dlg = gtk::FileDialog::builder()
        .title("Save image")
        .modal(true)
        .build();
    let st = state.clone();
    let cv = canvas.clone();
    let _lc = layers_cell.clone();
    dlg.save(Some(window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            if let Some(path) = file.path() {
                let mut g = st.borrow_mut();
                if let Err(e) = g.doc.save_png(&path) {
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
        if let Err(e) = g.doc.save_png(&path) {
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
        drop(g);
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

fn brightness_dialog(window: &libadwaita::ApplicationWindow, state: &SharedState, canvas: &CanvasCell) {
    let d = libadwaita::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Brightness / contrast")
        .default_width(360)
        .default_height(220)
        .build();

    let b_adj = gtk::Adjustment::new(0.0, -1.0, 1.0, 0.02, 0.1, 0.0);
    let c_adj = gtk::Adjustment::new(1.0, 0.1, 3.0, 0.02, 0.1, 0.0);
    let b_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&b_adj));
    let c_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&c_adj));
    b_scale.set_hexpand(true);
    c_scale.set_hexpand(true);

    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .spacing(12)
        .build();
    bx.append(&gtk::Label::new(Some("Brightness")));
    bx.append(&b_scale);
    bx.append(&gtk::Label::new(Some("Contrast")));
    bx.append(&c_scale);

    let apply = gtk::Button::with_label("Apply");
    let close = gtk::Button::with_label("Close");
    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    btn_row.append(&close);
    btn_row.append(&apply);
    bx.append(&btn_row);

    d.set_content(Some(&bx));

    let st = state.clone();
    let cv = canvas.clone();
    let dw = d.clone();
    apply.connect_clicked(move |_| {
        apply_brightness_contrast(
            &st,
            b_adj.value() as f32,
            c_adj.value() as f32,
        );
        queue_canvas(&cv);
    });
    close.connect_clicked(move |_| {
        dw.close();
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
    let color_btn_cell: ColorBtnCell = Rc::new(RefCell::new(None));
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
    drawing_area.add_tick_callback(move |_w, _clock| {
        let st = st_tick.borrow();
        if st.selection.is_some() || st.floating.is_some() {
            queue_canvas(&cv_tick);
        }
        glib::ControlFlow::Continue
    });

    setup_canvas_input(&drawing_area, &state, &canvas_cell, &color_btn_cell);

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
        st_al.borrow_mut().doc.add_layer();
        st_al.borrow_mut().history.clear();
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

    let tool_strings = gtk::StringList::new(&[
        "Brush",
        "Pixel",
        "Eraser",
        "Eyedropper",
        "Fill",
        "Line",
        "Rectangle",
        "Ellipse",
        "Select",
        "Move",
    ]);
    let tool_dd = gtk::DropDown::new(Some(tool_strings), gtk::Expression::NONE);
    *tool_dd_cell.borrow_mut() = Some(tool_dd.clone());
    tool_dd.set_hexpand(false);
    tool_dd.set_width_request(140);

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

    let color_btn = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    *color_btn_cell.borrow_mut() = Some(color_btn.clone());
    {
        let g = state.borrow();
        let rgba = gdk::RGBA::new(
            g.fg[0] as f32 / 255.0,
            g.fg[1] as f32 / 255.0,
            g.fg[2] as f32 / 255.0,
            g.fg[3] as f32 / 255.0,
        );
        color_btn.set_rgba(&rgba);
    }
    let st_c = state.clone();
    let cv_c = canvas_cell.clone();
    color_btn.connect_rgba_notify(move |btn| {
        let c = btn.rgba();
        let mut g = st_c.borrow_mut();
        g.fg = [
            (c.red() * 255.0).round().clamp(0.0, 255.0) as u8,
            (c.green() * 255.0).round().clamp(0.0, 255.0) as u8,
            (c.blue() * 255.0).round().clamp(0.0, 255.0) as u8,
            (c.alpha() * 255.0).round().clamp(0.0, 255.0) as u8,
        ];
        drop(g);
        queue_canvas(&cv_c);
    });

    let size_adj = gtk::Adjustment::new(8.0, 1.0, 256.0, 1.0, 8.0, 0.0);
    *size_adj_cell.borrow_mut() = Some(size_adj.clone());
    let size_spin = gtk::SpinButton::new(Some(&size_adj), 1.0, 0);
    size_spin.set_width_request(72);
    let st_sz = state.clone();
    size_adj.connect_value_changed(move |a| {
        st_sz.borrow_mut().brush_size = a.value();
    });

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
    toolbar.append(&make_row("Color", color_btn.upcast_ref()));
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    toolbar.append(&make_row("Size", size_spin.upcast_ref()));
    toolbar.append(&make_row("Hard", hard_scale.upcast_ref()));
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    toolbar.append(&make_row("Fill tol", tol_spin.upcast_ref()));
    toolbar.append(&fill_check);

    let editor = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .vexpand(true)
        .hexpand(true)
        .build();
    editor.append(&toolbar);
    editor.append(&drawing_area);

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&editor));
    split.set_end_child(Some(&layers_sidebar));
    split.set_resize_start_child(true);
    split.set_resize_end_child(false);
    split.set_shrink_start_child(true);
    split.set_shrink_end_child(false);

    let menu_model = build_menu(
        &window,
        &state,
        &layers_cell,
        &canvas_cell,
        &tool_dd_cell,
    );
    let menubar = gtk::PopoverMenuBar::from_model(Some(&menu_model));

    let header = libadwaita::HeaderBar::new();
    header.pack_start(&menubar);
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
                paste_clipboard_center(&st_k, &td_k);
                queue_canvas(&cv_k);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(key);

    window.present();
}

fn build_menu(
    window: &libadwaita::ApplicationWindow,
    state: &SharedState,
    layers_cell: &LayersCell,
    canvas: &CanvasCell,
    tool_dd_cell: &ToolDdCell,
) -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("New…"), Some("win.new"));
    file.append(Some("Open…"), Some("win.open"));
    file.append(Some("Save"), Some("win.save"));
    file.append(Some("Save As…"), Some("win.save_as"));
    file.append(Some("Quit"), Some("win.quit"));
    menu.append_submenu(Some("_File"), &file);

    let edit = gio::Menu::new();
    edit.append(Some("Undo"), Some("win.undo"));
    edit.append(Some("Redo"), Some("win.redo"));
    edit.append(Some("Cut"), Some("win.cut"));
    edit.append(Some("Copy"), Some("win.copy"));
    edit.append(Some("Paste"), Some("win.paste"));
    menu.append_submenu(Some("_Edit"), &edit);

    let image = gio::Menu::new();
    image.append(Some("Brightness / Contrast…"), Some("win.bc"));
    menu.append_submenu(Some("_Image"), &image);

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
    let paste_act = gio::SimpleAction::new("paste", None);
    paste_act.connect_activate(move |_, _| {
        paste_clipboard_center(&st, &td_p);
        queue_canvas(&cv);
    });
    app_add_action(window, &paste_act);

    let st = state.clone();
    let cv = canvas.clone();
    let w = window.clone();
    let bc_act = gio::SimpleAction::new("bc", None);
    bc_act.connect_activate(move |_, _| {
        brightness_dialog(&w, &st, &cv);
    });
    app_add_action(window, &bc_act);

    menu
}


fn app_add_action(window: &libadwaita::ApplicationWindow, action: &gio::SimpleAction) {
    window.add_action(action);
}
