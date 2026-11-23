#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Add,
}

impl BlendMode {
    pub fn as_str(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Add => "Add",
        }
    }

    pub fn dropdown_index(self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Multiply => 1,
            BlendMode::Add => 2,
        }
    }

    pub fn from_dropdown_index(idx: u32) -> Self {
        match idx {
            1 => BlendMode::Multiply,
            2 => BlendMode::Add,
            _ => BlendMode::Normal,
        }
    }

    pub fn ora_composite_op(self) -> &'static str {
        match self {
            BlendMode::Normal => "svg:src-over",
            BlendMode::Multiply => "svg:multiply",
            BlendMode::Add => "svg:plus",
        }
    }

    pub fn from_ora(s: &str) -> Self {
        match s {
            "svg:multiply" => BlendMode::Multiply,
            "svg:plus" => BlendMode::Add,
            _ => BlendMode::Normal,
        }
    }
}

/// Premultiplied RGBA8, `width * height * 4` bytes.
#[derive(Clone)]
pub struct Layer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    pub blend: BlendMode,
}

impl Layer {
    pub fn new(width: u32, height: u32, name: impl Into<String>) -> Self {
        let len = (width * height * 4) as usize;
        Self {
            width,
            height,
            pixels: vec![0u8; len],
            name: name.into(),
            opacity: 1.0,
            visible: true,
            blend: BlendMode::Normal,
        }
    }

    pub fn idx(&self, x: u32, y: u32) -> usize {
        ((y * self.width + x) * 4) as usize
    }

    pub fn pixel_premul(&self, x: u32, y: u32) -> [u8; 4] {
        let i = self.idx(x, y);
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

}
