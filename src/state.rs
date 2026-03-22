use crate::document::{Document, History};
use crate::tools::ToolKind;
use gdk_pixbuf::Pixbuf;

#[derive(Clone)]
pub struct FloatingSelection {
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
    pub x: f64,
    pub y: f64,
}

pub struct AppState {
    pub doc: Document,
    pub history: History,
    pub tool: ToolKind,
    pub fg: [u8; 4],
    pub brush_size: f64,
    pub brush_hardness: f64,
    pub fill_tolerance: u8,
    pub shape_filled: bool,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub selection: Option<(i32, i32, i32, i32)>,
    pub clipboard: Option<(i32, i32, Vec<u8>)>,
    pub floating: Option<FloatingSelection>,
    pub undo_snapshot: Option<(usize, Vec<u8>)>,
    pub last_doc_pos: Option<(f64, f64)>,
    pub drag_start_doc: Option<(f64, f64)>,
    /// Line / rectangle / ellipse drag: `(tool, x0, y0, x1, y1)` in document space (live preview).
    pub shape_drag_preview: Option<(ToolKind, f64, f64, f64, f64)>,
    /// When dragging a floating selection: `(pointer_doc_x - float_x, pointer_doc_y - float_y)`.
    pub move_grab_doc: Option<(f64, f64)>,
    pub modified: bool,
    pub tool_keybinds: Vec<(ToolKind, Option<char>)>,
    /// Most recently used foreground colors (straight RGBA), newest first; at most 4 kept.
    pub recent_colors: Vec<[u8; 4]>,
    /// Bumped when layer pixels, stack, visibility, opacity, or canvas size change (not selection/pan/zoom).
    pub document_visual_revision: u64,
    /// Cached full-document composite (premultiplied RGBA), valid when `composite_cache_at_revision == document_visual_revision`.
    pub composite_cache_premul: Vec<u8>,
    /// Scratch for straight RGBA while rebuilding the composite; moved into `glib::Bytes` when the cache is refreshed (often empty while cache hit).
    pub composite_cache_straight: Vec<u8>,
    /// Full-document composite for drawing; pixel data lives here between paints (not duplicated in `composite_cache_straight`).
    pub composite_cache_pixbuf: Option<Pixbuf>,
    pub composite_cache_at_revision: u64,
    /// While true, composite cache is not used (pixels change every event during brush/pixel/eraser stroke).
    pub brush_stroke_in_progress: bool,
}

impl AppState {
    pub fn default_tool_keybinds() -> Vec<(ToolKind, Option<char>)> {
        vec![
            (ToolKind::Brush, Some('b')),
            (ToolKind::Pixel, Some('p')),
            (ToolKind::Eraser, Some('e')),
            (ToolKind::Eyedropper, Some('k')),
            (ToolKind::Fill, Some('f')),
            (ToolKind::Line, Some('l')),
            (ToolKind::Rect, None),
            (ToolKind::Ellipse, None),
            (ToolKind::SelectRect, Some('s')),
            (ToolKind::Move, Some('m')),
            (ToolKind::Hand, Some('h')),
        ]
    }

    pub fn new() -> Self {
        Self {
            doc: Document::new(800, 600),
            history: History::new(),
            tool: ToolKind::Brush,
            fg: [0, 0, 0, 255],
            brush_size: 8.0,
            brush_hardness: 0.85,
            fill_tolerance: 32,
            shape_filled: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            selection: None,
            clipboard: None,
            floating: None,
            undo_snapshot: None,
            last_doc_pos: None,
            drag_start_doc: None,
            shape_drag_preview: None,
            move_grab_doc: None,
            modified: false,
            tool_keybinds: Self::default_tool_keybinds(),
            recent_colors: Vec::new(),
            document_visual_revision: 0,
            composite_cache_premul: Vec::new(),
            composite_cache_straight: Vec::new(),
            composite_cache_pixbuf: None,
            composite_cache_at_revision: u64::MAX,
            brush_stroke_in_progress: false,
        }
    }

    /// Invalidate composite cache (call after any change that affects flattened pixels or layer stack).
    pub fn bump_document_revision(&mut self) {
        self.document_visual_revision = self.document_visual_revision.wrapping_add(1);
    }

    /// Drop GPU-adjacent composite caches on application shutdown so heap blocks are freed before exit.
    pub fn release_drawing_caches(&mut self) {
        self.composite_cache_pixbuf = None;
        self.composite_cache_premul.clear();
        self.composite_cache_premul.shrink_to_fit();
        self.composite_cache_straight.clear();
        self.composite_cache_straight.shrink_to_fit();
        self.composite_cache_at_revision = u64::MAX;
    }

    pub fn widget_to_doc(&self, wx: f64, wy: f64) -> (f64, f64) {
        ((wx - self.pan_x) / self.zoom, (wy - self.pan_y) / self.zoom)
    }

    pub fn begin_stroke_undo(&mut self) {
        if let Some(layer) = self.doc.active_layer_ref() {
            self.undo_snapshot = Some((self.doc.active_layer, layer.pixels.clone()));
        }
    }

    pub fn commit_stroke_undo(&mut self) {
        if let Some((idx, before)) = self.undo_snapshot.take() {
            if let Some(layer) = self.doc.layers.get(idx) {
                if layer.pixels != before {
                    self.history.commit_change(idx, before);
                    self.modified = true;
                    self.bump_document_revision();
                }
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.history.undo(&mut self.doc) {
            self.modified = true;
            self.bump_document_revision();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if self.history.redo(&mut self.doc) {
            self.modified = true;
            self.bump_document_revision();
            true
        } else {
            false
        }
    }

    pub fn normalize_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> (i32, i32, i32, i32) {
        let min_x = x0.min(x1).floor() as i32;
        let min_y = y0.min(y1).floor() as i32;
        let max_x = x0.max(x1).ceil() as i32;
        let max_y = y0.max(y1).ceil() as i32;
        let w = (max_x - min_x).max(1);
        let h = (max_y - min_y).max(1);
        (min_x, min_y, w, h)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
