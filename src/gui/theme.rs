use gpui::Rgba;

const fn rgb(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

pub const fn alpha(color: Rgba, opacity: f32) -> Rgba {
    Rgba {
        a: opacity,
        ..color
    }
}

pub const BG: Rgba = rgb(0x11111b);
pub const MANTLE: Rgba = rgb(0x181825);
pub const SURFACE: Rgba = rgb(0x1e1e2e);
pub const SURFACE_2: Rgba = rgb(0x313244);
pub const LINE: Rgba = rgb(0x292938);
pub const LINE_STRONG: Rgba = rgb(0x45475a);
pub const TEXT: Rgba = rgb(0xcdd6f4);
pub const SUBTEXT: Rgba = rgb(0xbac2de);
pub const MUTED: Rgba = rgb(0x9399b2);
pub const BLUE: Rgba = rgb(0x89b4fa);
pub const RED: Rgba = rgb(0xf38ba8);
pub const GREEN: Rgba = rgb(0xa6e3a1);
pub const TEAL: Rgba = rgb(0x94e2d5);
pub const MAUVE: Rgba = rgb(0xcba6f7);

pub const FONT_MONO: &str = "IBM Plex Mono";
pub const FONT_DISPLAY: &str = "Geist";
