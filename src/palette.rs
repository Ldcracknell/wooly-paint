//! Named color palettes, hex import/export (Lospec / Pico-8 style: `#RRGGBB` or plain `RRGGBB` lines), and persistence helpers.

use serde::{Deserialize, Serialize};

/// Built-in sidebar palette name (always kept at index0; not deletable).
pub const BUILTIN_PALETTE_NAME: &str = "Default";

/// Trailing row in the sidebar palette `DropDown`; selecting it creates a new palette.
pub const NEW_PALETTE_DROPDOWN_LABEL: &str = "+ New palette";

/// Hard cap per palette to keep the UI responsive.
pub const MAX_COLORS_PER_PALETTE: usize = 512;

/// Straight RGBA presets matching the original sidebar defaults.
pub const BUILTIN_SWATCHES: &[[u8; 4]] = &[
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

/// Lexaloffle Pico-8 display palette (16 colours).
pub const PICO8_COLORS: &[[u8; 4]] = &[
    [0, 0, 0, 255],
    [29, 43, 83, 255],
    [126, 37, 83, 255],
    [0, 135, 81, 255],
    [171, 82, 54, 255],
    [95, 87, 79, 255],
    [194, 195, 199, 255],
    [255, 241, 232, 255],
    [255, 0, 77, 255],
    [255, 163, 0, 255],
    [255, 236, 39, 255],
    [0, 231, 86, 255],
    [41, 173, 255, 255],
    [131, 118, 156, 255],
    [255, 119, 168, 255],
    [255, 204, 170, 255],
];

/// ENDESGA 32 (order matches common `.palette` / Lospec exports).
pub const ENDESGA_32_COLORS: &[[u8; 4]] = &[
    [190, 74, 47, 255],
    [215, 118, 67, 255],
    [234, 212, 170, 255],
    [228, 166, 114, 255],
    [184, 111, 80, 255],
    [115, 62, 57, 255],
    [62, 39, 49, 255],
    [162, 38, 51, 255],
    [228, 59, 68, 255],
    [247, 118, 34, 255],
    [254, 174, 52, 255],
    [254, 231, 97, 255],
    [99, 199, 77, 255],
    [62, 137, 72, 255],
    [38, 92, 66, 255],
    [25, 60, 62, 255],
    [18, 78, 137, 255],
    [0, 153, 219, 255],
    [44, 232, 245, 255],
    [255, 255, 255, 255],
    [192, 203, 220, 255],
    [139, 155, 180, 255],
    [90, 105, 136, 255],
    [58, 68, 102, 255],
    [38, 43, 68, 255],
    [24, 20, 37, 255],
    [255, 0, 68, 255],
    [104, 56, 108, 255],
    [181, 80, 136, 255],
    [246, 117, 122, 255],
    [232, 183, 150, 255],
    [194, 133, 105, 255],
];

/// Sweetie 16 (GrafxKid).
pub const SWEETIE_16_COLORS: &[[u8; 4]] = &[
    [27, 38, 50, 255],
    [72, 59, 58, 255],
    [158, 40, 53, 255],
    [229, 59, 68, 255],
    [239, 101, 85, 255],
    [252, 158, 79, 255],
    [255, 210, 142, 255],
    [153, 229, 80, 255],
    [52, 190, 91, 255],
    [69, 175, 110, 255],
    [36, 128, 137, 255],
    [30, 111, 124, 255],
    [38, 92, 100, 255],
    [44, 232, 244, 255],
    [255, 255, 255, 255],
    [75, 105, 47, 255],
];

/// DawnBringer 16.
pub const DAWNBRINGER_16_COLORS: &[[u8; 4]] = &[
    [20, 12, 28, 255],
    [68, 36, 52, 255],
    [48, 52, 109, 255],
    [78, 74, 78, 255],
    [133, 76, 48, 255],
    [208, 70, 72, 255],
    [89, 125, 206, 255],
    [210, 125, 44, 255],
    [133, 149, 161, 255],
    [109, 170, 44, 255],
    [210, 170, 153, 255],
    [69, 40, 60, 255],
    [126, 37, 83, 255],
    [223, 113, 38, 255],
    [25, 60, 62, 255],
    [254, 231, 97, 255],
];

/// Classic 4-colour Game Boy-style greens.
pub const GAMEBOY_4_COLORS: &[[u8; 4]] = &[
    [15, 56, 15, 255],
    [48, 98, 48, 255],
    [139, 172, 15, 255],
    [155, 188, 15, 255],
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamedPalette {
    pub name: String,
    pub colors: Vec<[u8; 4]>,
}

impl NamedPalette {
    pub fn new(name: impl Into<String>, colors: Vec<[u8; 4]>) -> Self {
        Self {
            name: name.into(),
            colors,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteBook {
    pub entries: Vec<NamedPalette>,
    pub active: usize,
}

impl PaletteBook {
    pub fn new_builtin_only() -> Self {
        Self {
            entries: vec![
                NamedPalette::new(BUILTIN_PALETTE_NAME, BUILTIN_SWATCHES.to_vec()),
                NamedPalette::new("Pico-8", PICO8_COLORS.to_vec()),
                NamedPalette::new("Endesga 32", ENDESGA_32_COLORS.to_vec()),
                NamedPalette::new("Sweetie 16", SWEETIE_16_COLORS.to_vec()),
                NamedPalette::new("DawnBringer 16", DAWNBRINGER_16_COLORS.to_vec()),
                NamedPalette::new("Game Boy", GAMEBOY_4_COLORS.to_vec()),
            ],
            active: 0,
        }
    }

    /// Restore from settings; falls back to built-in if data is unusable.
    pub fn from_loaded(mut entries: Vec<NamedPalette>, active: usize) -> Self {
        entries.retain(|e| !e.name.is_empty() && !e.colors.is_empty());
        for e in &mut entries {
            e.colors.truncate(MAX_COLORS_PER_PALETTE);
        }
        if entries.is_empty() {
            return Self::new_builtin_only();
        }
        let active = active.min(entries.len() - 1);
        Self { entries, active }
    }

    /// Append any built-in named palettes that are not already in the book (for upgrades / old settings files).
    pub fn merge_missing_builtin_presets(&mut self) -> bool {
        let builtins = Self::new_builtin_only().entries;
        let mut names: std::collections::HashSet<String> =
            self.entries.iter().map(|e| e.name.clone()).collect();
        let mut added = false;
        for bp in builtins {
            if names.contains(&bp.name) {
                continue;
            }
            names.insert(bp.name.clone());
            self.entries.push(bp);
            added = true;
        }
        self.clamp_active();
        added
    }

    pub fn active_palette(&self) -> &NamedPalette {
        &self.entries[self.active]
    }

    pub fn active_colors(&self) -> &[[u8; 4]] {
        &self.entries[self.active].colors
    }

    /// Append a swatch to the active palette. Returns `false` if the palette is at capacity.
    pub fn append_color_to_active(&mut self, rgba: [u8; 4]) -> bool {
        self.clamp_active();
        let pal = &mut self.entries[self.active];
        if pal.colors.len() >= MAX_COLORS_PER_PALETTE {
            return false;
        }
        pal.colors.push(rgba);
        true
    }

    pub fn clamp_active(&mut self) {
        if self.entries.is_empty() {
            *self = Self::new_builtin_only();
            return;
        }
        self.active = self.active.min(self.entries.len() - 1);
    }

    /// Insert after import or duplicate. `name` is trimmed; empty becomes `"Imported"`.
    pub fn push_palette(&mut self, name: impl AsRef<str>, colors: Vec<[u8; 4]>) {
        let name = name.as_ref().trim();
        let name = if name.is_empty() {
            "Imported".to_string()
        } else {
            name.to_string()
        };
        self.entries.push(NamedPalette::new(name, colors));
        self.active = self.entries.len() - 1;
        self.clamp_active();
    }

    pub fn duplicate_entry(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        let base = self.entries[index].name.clone();
        let colors = self.entries[index].colors.clone();
        let name = format!("Copy of {base}");
        self.entries.push(NamedPalette::new(name, colors));
        self.active = self.entries.len() - 1;
        true
    }

    pub fn new_empty_swatch(&mut self) {
        self.entries
            .push(NamedPalette::new("New palette", vec![[0, 0, 0, 255]]));
        self.active = self.entries.len() - 1;
    }

    /// Remove palette at index. Index 0 (built-in slot) cannot be removed.
    pub fn remove_at(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.entries.len() || self.entries.len() <= 1 {
            return false;
        }
        self.entries.remove(index);
        if self.active >= self.entries.len() {
            self.active = self.entries.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }
        true
    }

    pub fn rename(&mut self, index: usize, new_name: &str) -> bool {
        let new_name = new_name.trim();
        if new_name.is_empty() || index >= self.entries.len() {
            return false;
        }
        self.entries[index].name = new_name.to_string();
        true
    }

    /// Drop one swatch from a palette. Returns `false` if the palette would become empty.
    pub fn remove_color_at(&mut self, palette_index: usize, color_index: usize) -> bool {
        if palette_index >= self.entries.len() {
            return false;
        }
        let colors = &mut self.entries[palette_index].colors;
        if color_index >= colors.len() || colors.len() <= 1 {
            return false;
        }
        colors.remove(color_index);
        true
    }
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + (b - b'a')),
        b'A'..=b'F' => Some(10 + (b - b'A')),
        _ => None,
    }
}

fn parse_hex_nibble_pair(hi: u8, lo: u8) -> Option<u8> {
    Some(hex_value(hi)? << 4 | hex_value(lo)?)
}

/// Expand `#RGB` to `[r,g,b]`.
fn rgb12_to_rgb24(r: u8, g: u8, b: u8) -> [u8; 3] {
    [r << 4 | r, g << 4 | g, b << 4 | b]
}

/// Parse `RGB`, `RGBA`, `RRGGBB`, or `RRGGBBAA` from a byte slice of ASCII hex digits (no `#`).
fn parse_hex_color_digits(slice: &[u8]) -> Option<[u8; 4]> {
    if !slice.iter().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let len = slice.len();
    match len {
        3 => {
            let r = hex_value(slice[0])?;
            let g = hex_value(slice[1])?;
            let b = hex_value(slice[2])?;
            let [r, g, b] = rgb12_to_rgb24(r, g, b);
            Some([r, g, b, 255])
        }
        4 => {
            let r = hex_value(slice[0])?;
            let g = hex_value(slice[1])?;
            let b = hex_value(slice[2])?;
            let a = hex_value(slice[3])?;
            let [r, g, b] = rgb12_to_rgb24(r, g, b);
            Some([r, g, b, a << 4 | a])
        }
        6 => Some([
            parse_hex_nibble_pair(slice[0], slice[1])?,
            parse_hex_nibble_pair(slice[2], slice[3])?,
            parse_hex_nibble_pair(slice[4], slice[5])?,
            255,
        ]),
        8 => Some([
            parse_hex_nibble_pair(slice[0], slice[1])?,
            parse_hex_nibble_pair(slice[2], slice[3])?,
            parse_hex_nibble_pair(slice[4], slice[5])?,
            parse_hex_nibble_pair(slice[6], slice[7])?,
        ]),
        _ => None,
    }
}

fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = s.split_first() {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = s.split_last() {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// One token: optional `#` prefix, then 3/4/6/8 hex digits.
fn color_from_token(mut token: &[u8]) -> Option<[u8; 4]> {
    token = trim_ascii(token);
    if token.first() == Some(&b'#') {
        token = &token[1..];
    }
    token = trim_ascii(token);
    if token.is_empty() {
        return None;
    }
    parse_hex_color_digits(token)
}

fn colors_from_line(line: &[u8]) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    for raw in line.split(|b| *b == b',' || b.is_ascii_whitespace()) {
        let raw = trim_ascii(raw);
        if raw.is_empty() {
            continue;
        }
        if let Some(rgba) = color_from_token(raw) {
            out.push(rgba);
        }
    }
    out
}

/// Parse hex palette text: lines of `#RRGGBB` or plain `RRGGBB` (Pico-8 export), optional `#RRGGBBAA` / `RRGGBBAA`, `#RGB`, `;` comments, comma/whitespace-separated tokens.
pub fn parse_hex_palette_text(text: &str) -> Result<Vec<[u8; 4]>, String> {
    let mut colors = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(';') {
            continue;
        }
        let bytes = line.as_bytes();
        for rgba in colors_from_line(bytes) {
            if colors.len() >= MAX_COLORS_PER_PALETTE {
                return Err(format!(
                    "Too many colors (max {MAX_COLORS_PER_PALETTE})"
                ));
            }
            colors.push(rgba);
        }
    }
    if colors.is_empty() {
        Err("No hex colors found (expected lines like #RRGGBB or RRGGBB)".to_string())
    } else {
        Ok(colors)
    }
}

/// Parse a single color from a text field (optional `#`, one token; ignores extra whitespace).
pub fn parse_hex_color_input(text: &str) -> Option<[u8; 4]> {
    let line = text.trim();
    if line.is_empty() {
        return None;
    }
    colors_from_line(line.as_bytes()).into_iter().next()
}

/// Export as Lospec-friendly hex: one `#RRGGBB` per line. Uses opaque alpha only; otherwise `#RRGGBBAA`.
pub fn format_hex_palette(colors: &[[u8; 4]]) -> String {
    let mut s = String::new();
    for c in colors {
        if c[3] == 255 {
            s.push_str(&format!("#{:02x}{:02x}{:02x}\n", c[0], c[1], c[2]));
        } else {
            s.push_str(&format!(
                "#{:02x}{:02x}{:02x}{:02x}\n",
                c[0], c[1], c[2], c[3]
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lospec_pico8_style() {
        let text = "#000000\n#1D2B53\n#7E2553\n";
        let v = parse_hex_palette_text(text).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], [0, 0, 0, 255]);
        assert_eq!(v[1], [0x1d, 0x2b, 0x53, 255]);
    }

    #[test]
    fn pico8_bare_hex_lines() {
        let text = "000000\n1D2B53\n7E2553\n";
        let v = parse_hex_palette_text(text).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], [0, 0, 0, 255]);
        assert_eq!(v[1], [0x1d, 0x2b, 0x53, 255]);
        assert_eq!(v[2], [0x7e, 0x25, 0x53, 255]);
    }

    #[test]
    fn rgb_short_form() {
        let v = parse_hex_palette_text("#f0a\n").unwrap();
        assert_eq!(v[0], [0xff, 0x00, 0xaa, 255]);
    }

    #[test]
    fn with_alpha() {
        let v = parse_hex_palette_text("#10203040\n").unwrap();
        assert_eq!(v[0], [0x10, 0x20, 0x30, 0x40]);
    }

    #[test]
    fn single_hex_input_no_hash() {
        assert_eq!(
            parse_hex_color_input("FF004D"),
            Some([255, 0, 77, 255])
        );
        assert_eq!(parse_hex_color_input("  #abc "), Some([0xaa, 0xbb, 0xcc, 255]));
    }

    #[test]
    fn builtin_book_includes_presets() {
        let book = PaletteBook::new_builtin_only();
        assert_eq!(book.entries.len(), 6);
        assert_eq!(book.entries[1].name, "Pico-8");
        assert_eq!(book.entries[1].colors.len(), 16);
        assert_eq!(book.entries[2].colors.len(), 32);
        assert_eq!(book.entries[2].name, "Endesga 32");
    }

    #[test]
    fn merge_presets_after_old_save() {
        let mut book = PaletteBook::from_loaded(
            vec![NamedPalette::new("Default", vec![[0, 0, 0, 255]])],
            0,
        );
        assert_eq!(book.entries.len(), 1);
        assert!(book.merge_missing_builtin_presets());
        assert_eq!(book.entries.len(), 6);
        assert!(book.entries.iter().any(|e| e.name == "Pico-8"));
        assert!(!book.merge_missing_builtin_presets());
    }
}
