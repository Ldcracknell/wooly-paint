//! Per-tool cursors: Lucide icons (ISC) rasterized in `build.rs` — see `assets/cursors/THIRD_PARTY_NOTICES.txt`.
//! Hotspots are generated to match SVG geometry so the active point aligns with painting / sampling.

use crate::tools::ToolKind;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::WidgetExt;
use std::cell::RefCell;

include!(concat!(env!("OUT_DIR"), "/cursor_hotspots.rs"));

pub struct ToolCursorPack {
    brush: gdk::Cursor,
    pixel: gdk::Cursor,
    eraser: gdk::Cursor,
    eyedropper: gdk::Cursor,
    fill: gdk::Cursor,
    line: gdk::Cursor,
    rect: gdk::Cursor,
    ellipse: gdk::Cursor,
    select: gdk::Cursor,
    r#move: gdk::Cursor,
    hand: gdk::Cursor,
    grabbing: gdk::Cursor,
}

fn named_or(name: &str, fb: &gdk::Cursor) -> gdk::Cursor {
    gdk::Cursor::from_name(name, Some(fb)).unwrap_or_else(|| fb.clone())
}

fn tex(bytes: &'static [u8], hotspot: (i32, i32), fb: &gdk::Cursor) -> gdk::Cursor {
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)).expect("cursor png");
    gdk::Cursor::from_texture(&texture, hotspot.0, hotspot.1, Some(fb))
}

impl ToolCursorPack {
    fn new() -> Self {
        let fb = gdk::Cursor::from_name("default", None).expect("default cursor");
        let grab_theme = named_or("grab", &fb);
        let grabbing = named_or("grabbing", &grab_theme);

        Self {
            brush: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/brush.png")),
                HOTSPOT_BRUSH,
                &fb,
            ),
            pixel: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/pixel.png")),
                HOTSPOT_PIXEL,
                &fb,
            ),
            eraser: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/eraser.png")),
                HOTSPOT_ERASER,
                &fb,
            ),
            eyedropper: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/eyedropper.png")),
                HOTSPOT_EYEDROPPER,
                &fb,
            ),
            fill: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/fill.png")),
                HOTSPOT_FILL,
                &fb,
            ),
            line: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/line.png")),
                HOTSPOT_LINE,
                &fb,
            ),
            rect: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/rect.png")),
                HOTSPOT_RECT,
                &fb,
            ),
            ellipse: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/ellipse.png")),
                HOTSPOT_ELLIPSE,
                &fb,
            ),
            select: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/select.png")),
                HOTSPOT_SELECT,
                &fb,
            ),
            r#move: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/move.png")),
                HOTSPOT_MOVE,
                &fb,
            ),
            hand: tex(
                include_bytes!(concat!(env!("OUT_DIR"), "/hand.png")),
                HOTSPOT_HAND,
                &fb,
            ),
            grabbing,
        }
    }

    fn cursor_for_tool(&self, tool: ToolKind) -> &gdk::Cursor {
        match tool {
            ToolKind::Brush => &self.brush,
            ToolKind::Pixel => &self.pixel,
            ToolKind::Eraser => &self.eraser,
            ToolKind::Eyedropper => &self.eyedropper,
            ToolKind::Fill => &self.fill,
            ToolKind::Line => &self.line,
            ToolKind::Rect => &self.rect,
            ToolKind::Ellipse => &self.ellipse,
            ToolKind::SelectRect => &self.select,
            ToolKind::Move => &self.r#move,
            ToolKind::Hand => &self.hand,
        }
    }
}

thread_local! {
    static PACK: RefCell<Option<ToolCursorPack>> = const { RefCell::new(None) };
}

fn with_pack<R>(f: impl FnOnce(&ToolCursorPack) -> R) -> R {
    PACK.with(|cell| {
        let mut b = cell.borrow_mut();
        if b.is_none() {
            *b = Some(ToolCursorPack::new());
        }
        f(b.as_ref().expect("just set"))
    })
}

/// Normal tool cursor (not panning / dragging the canvas with hand or middle mouse).
pub fn sync_canvas_tool_cursor(canvas: &gtk::DrawingArea, tool: ToolKind) {
    with_pack(|p| {
        canvas.set_cursor(Some(p.cursor_for_tool(tool)));
    });
}

pub fn set_canvas_grabbing(canvas: &gtk::DrawingArea) {
    with_pack(|p| {
        canvas.set_cursor(Some(&p.grabbing));
    });
}
