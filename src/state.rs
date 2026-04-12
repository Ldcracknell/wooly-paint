use crate::document::{composite_layers_prefix_into, Document, History};
use crate::palette::PaletteBook;
use crate::tools::ToolKind;
use gdk_pixbuf::Pixbuf;
use gtk::cairo::ImageSurface;
use std::path::PathBuf;

#[derive(Clone)]
pub struct FloatingSelection {
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
    pub x: f64,
    pub y: f64,
    /// Clockwise degrees (cairo convention), pivot at bitmap center.
    pub angle_deg: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl FloatingSelection {
    pub fn new_pasted(w: i32, h: i32, data: Vec<u8>, x: f64, y: f64) -> Self {
        Self {
            w,
            h,
            data,
            x,
            y,
            angle_deg: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            flip_h: false,
            flip_v: false,
        }
    }
}

/// Rectangular marquee or magic-wand (connected) region in document space.
#[derive(Clone)]
pub enum Selection {
    Rect(i32, i32, i32, i32),
    /// `mask.len() == width * height`; non-zero = selected (same size as the document when created).
    Region {
        width: u32,
        height: u32,
        mask: Vec<u8>,
        /// When set (e.g. magic wand), tight bounds of `mask` to skip a full-mask scan.
        tight_bbox: Option<(i32, i32, i32, i32)>,
    },
}

impl Selection {
    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        let xi = x.floor() as i32;
        let yi = y.floor() as i32;
        match self {
            Selection::Rect(sx, sy, sw, sh) => {
                xi >= *sx && yi >= *sy && xi < sx + sw && yi < sy + sh
            }
            Selection::Region {
                width,
                height,
                mask,
                ..
            } => {
                if xi < 0 || yi < 0 || xi >= *width as i32 || yi >= *height as i32 {
                    return false;
                }
                let idx = (yi as u32 * width + xi as u32) as usize;
                mask.get(idx).copied().unwrap_or(0) != 0
            }
        }
    }
}

/// Active drag on the floating selection (handles on the marquee, not sidebar).
#[derive(Clone, Copy, Debug)]
pub enum FloatingDrag {
    Move {
        grab_off_x: f64,
        grab_off_y: f64,
    },
    Rotate {
        base_angle_deg: f64,
        start_pointer_rad: f64,
    },
    ResizeCorner {
        dragged_corner: u8,
        anchor_doc: (f64, f64),
    },
    ResizeEdge {
        edge: u8,
        anchor_doc: (f64, f64),
    },
}

/// Left vs right mouse button color slots (primary / secondary click).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ColorSlot {
    #[default]
    Left,
    Right,
}

pub struct AppState {
    pub doc: Document,
    pub history: History,
    pub tool: ToolKind,
    /// Left mouse button / primary drawing color.
    pub fg: [u8; 4],
    /// Right mouse button / secondary drawing color.
    pub bg: [u8; 4],
    /// GDK button id for the active canvas drag (`BUTTON_PRIMARY` / `BUTTON_SECONDARY`).
    pub pointer_drag_button: u32,
    /// Which color the hue/SV picker edits.
    pub picker_target: ColorSlot,
    /// Hue for the SV plane and hue strip, 0..1.
    pub picker_hue: f64,
    pub brush_size: f64,
    pub brush_hardness: f64,
    pub fill_tolerance: u8,
    pub shape_filled: bool,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    /// When true, draw 1×1 pixel cell lines over the canvas (document space).
    pub show_pixel_grid: bool,
    pub selection: Option<Selection>,
    pub clipboard: Option<(i32, i32, Vec<u8>)>,
    pub floating: Option<FloatingSelection>,
    pub undo_snapshot: Option<(usize, Vec<u8>)>,
    pub last_doc_pos: Option<(f64, f64)>,
    pub drag_start_doc: Option<(f64, f64)>,
    /// Line / rectangle / ellipse drag: `(tool, x0, y0, x1, y1)` in document space (live preview).
    pub shape_drag_preview: Option<(ToolKind, f64, f64, f64, f64)>,
    /// When dragging a floating selection: `(pointer_doc_x - float_x, pointer_doc_y - float_y)`.
    pub move_grab_doc: Option<(f64, f64)>,
    /// Move / rotate / resize via on-canvas handles (Move tool + floating).
    pub floating_drag: Option<FloatingDrag>,
    pub modified: bool,
    pub tool_keybinds: Vec<(ToolKind, Option<char>)>,
    /// Most recently used foreground colors (straight RGBA), newest first; at most 4 kept.
    pub recent_colors: Vec<[u8; 4]>,
    /// Named palettes (sidebar); first entry is the built-in default.
    pub palette_book: PaletteBook,
    /// Recently opened documents (paths), newest first; at most 5 kept.
    pub recent_files: Vec<PathBuf>,
    /// Bumped when layer pixels, stack, visibility, opacity, or canvas size change (not selection/pan/zoom).
    pub document_visual_revision: u64,
    /// Cached full-document composite (premultiplied RGBA), valid when `composite_cache_at_revision == document_visual_revision`.
    pub composite_cache_premul: Vec<u8>,
    /// Cairo `ImageSurface` for painting the flattened document (BGRA premul); pixels updated from `composite_cache_premul`.
    pub composite_cache_surface: Option<ImageSurface>,
    pub composite_cache_at_revision: u64,
    /// Straight RGBA scratch for rebuilding [`Self::floating_pixbuf_cache`].
    pub floating_straight_scratch: Vec<u8>,
    pub floating_pixbuf_cache: Option<Pixbuf>,
    pub floating_pixbuf_key: Option<(usize, usize, i32, i32)>,
    /// While true, composite cache is not used (pixels change every event during brush/pixel/eraser stroke).
    pub brush_stroke_in_progress: bool,
    /// During brush/pixel/eraser stroke: flattened premul RGBA of layers strictly below
    /// [`Document::active_layer`] at stroke start. Used to avoid recompositing the full stack each frame.
    pub stroke_composite_below: Option<Vec<u8>>,
    pub stroke_composite_active_layer: usize,
    pub stroke_composite_doc_wh: (u32, u32),
}

impl AppState {
    /// Paint color for the active pointer drag (left → [`Self::fg`], right → [`Self::bg`]).
    pub fn active_paint_color(&self) -> [u8; 4] {
        if self.pointer_drag_button == 3 {
            self.bg
        } else {
            self.fg
        }
    }

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
            (ToolKind::MagicSelect, Some('w')),
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
            bg: [255, 255, 255, 255],
            pointer_drag_button: 1,
            picker_target: ColorSlot::Left,
            picker_hue: 0.0,
            brush_size: 8.0,
            brush_hardness: 0.1,
            fill_tolerance: 32,
            shape_filled: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            show_pixel_grid: false,
            selection: None,
            clipboard: None,
            floating: None,
            undo_snapshot: None,
            last_doc_pos: None,
            drag_start_doc: None,
            shape_drag_preview: None,
            move_grab_doc: None,
            floating_drag: None,
            modified: false,
            tool_keybinds: Self::default_tool_keybinds(),
            recent_colors: Vec::new(),
            palette_book: PaletteBook::new_builtin_only(),
            recent_files: Vec::new(),
            document_visual_revision: 0,
            composite_cache_premul: Vec::new(),
            composite_cache_surface: None,
            composite_cache_at_revision: u64::MAX,
            floating_straight_scratch: Vec::new(),
            floating_pixbuf_cache: None,
            floating_pixbuf_key: None,
            brush_stroke_in_progress: false,
            stroke_composite_below: None,
            stroke_composite_active_layer: 0,
            stroke_composite_doc_wh: (0, 0),
        }
    }

    /// Call after [`Self::begin_stroke_undo`] on brush/pixel/eraser press, before mutating the active layer.
    pub fn capture_stroke_composite_below(&mut self) {
        let w = self.doc.width;
        let h = self.doc.height;
        let len = (w * h * 4) as usize;
        let active = self.doc.active_layer;
        let mut v = vec![0u8; len];
        composite_layers_prefix_into(&mut v, w, h, &self.doc.layers, active);
        self.stroke_composite_below = Some(v);
        self.stroke_composite_active_layer = active;
        self.stroke_composite_doc_wh = (w, h);
    }

    pub fn clear_stroke_composite_below(&mut self) {
        self.stroke_composite_below = None;
        self.stroke_composite_doc_wh = (0, 0);
    }

    /// True when incremental stroke compositing matches the current document and active layer.
    pub fn stroke_composite_below_valid(&self) -> bool {
        let (dw, dh) = (self.doc.width, self.doc.height);
        let expected = (dw * dh * 4) as usize;
        let Some(buf) = self.stroke_composite_below.as_ref() else {
            return false;
        };
        !buf.is_empty()
            && buf.len() == expected
            && self.stroke_composite_doc_wh == (dw, dh)
            && self.stroke_composite_active_layer == self.doc.active_layer
    }

    /// Invalidate composite cache (call after any change that affects flattened pixels or layer stack).
    pub fn bump_document_revision(&mut self) {
        self.document_visual_revision = self.document_visual_revision.wrapping_add(1);
    }

    /// Drop GPU-adjacent composite caches on application shutdown so heap blocks are freed before exit.
    pub fn release_drawing_caches(&mut self) {
        self.clear_stroke_composite_below();
        self.composite_cache_surface = None;
        self.composite_cache_premul.clear();
        self.composite_cache_premul.shrink_to_fit();
        self.composite_cache_at_revision = u64::MAX;
        self.floating_straight_scratch.clear();
        self.floating_straight_scratch.shrink_to_fit();
        self.floating_pixbuf_cache = None;
        self.floating_pixbuf_key = None;
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
