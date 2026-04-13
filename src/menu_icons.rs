//! Lucide menu icons rasterized in `build.rs` (same approach as `tool_cursors::tool_dropdown_icon_texture`).

use gtk::gdk;
use gtk::glib;

pub fn menu_bar_icon_texture(alias: &str, ui_dark: bool) -> gdk::Texture {
    let bytes = glib::Bytes::from_static(menu_icon_png(alias, ui_dark));
    gdk::Texture::from_bytes(&bytes).expect("menu icon png")
}

fn menu_icon_png(alias: &str, ui_dark: bool) -> &'static [u8] {
    macro_rules! icons {
        ($($name:literal),* $(,)?) => {
            match (alias, ui_dark) {
                $(
                    ($name, true) => include_bytes!(concat!(env!("OUT_DIR"), "/menu/", $name, "_dark.png")),
                    ($name, false) => include_bytes!(concat!(env!("OUT_DIR"), "/menu/", $name, "_light.png")),
                )*
                _ => panic!("unknown menu icon alias: {alias}"),
            }
        };
    }
    icons!(
        "file",
        "new",
        "open",
        "recent",
        "save",
        "save_as",
        "canvas",
        "resize",
        "flip_x",
        "flip_y",
        "rotate",
        "grid",
        "settings",
        "keybinds",
        "updates",
        "theme",
        "theme_default",
        "theme_light",
        "theme_dark",
        "palettes",
        "import_hex",
        "export_hex",
        "manage_palettes",
        "image",
    )
}
