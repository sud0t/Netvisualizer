//! Colours, sizes and looks, declared once. Everything the viewer draws
//! takes its colour from this table, so a palette change is made here and
//! nowhere else. The looks at the bottom are the shapes the UI is built
//! from (ui/shape.rs paints them).
//!
//! The interface is a terminal: one monospace face, phosphor green on
//! near-black boxes with hard corners and a faint glow, scanlines over the
//! whole window. The role colours (cyan, amber, violet, blue) stay as they
//! are so the host list and the legend still match the spheres.

use bevy::prelude::Color;

use crate::ui::shape::Look;

// --- The scene --------------------------------------------------------------

pub const SCENE_BG: Color = Color::srgb_u8(0x0b, 0x0f, 0x17);
pub const GRID_LINE: Color = Color::srgb_u8(0x23, 0x33, 0x50);
pub const GRID_AXIS: Color = Color::srgb_u8(0x3a, 0x52, 0x80);
pub const LINK: Color = Color::srgb_u8(0x3d, 0x5a, 0x85);
pub const SELECTION: Color = Color::WHITE;

pub const ROLE_SCANNER: Color = Color::srgb_u8(0x22, 0xd3, 0xee);
pub const ROLE_GATEWAY: Color = Color::srgb_u8(0xf5, 0x9e, 0x0b);
pub const ROLE_ROUTER: Color = Color::srgb_u8(0xc0, 0x84, 0xfc);
pub const ROLE_HOST: Color = Color::srgb_u8(0x60, 0xa5, 0xfa);
pub const ROLE_DOWN: Color = Color::srgb_u8(0x6b, 0x72, 0x80);

/// Opacity of the grid floor's lines.
pub const GRID_OPACITY: f32 = 0.6;
/// Opacity of link lines.
pub const LINK_OPACITY: f32 = 0.6;

// --- Text -------------------------------------------------------------------

/// The phosphor: the accent, and the text that is lit (the brand, a toggle
/// that is on, Scan).
pub const ACCENT: Color = Color::srgb_u8(0x39, 0xff, 0x88);
pub const ACCENT_TEXT: Color = ACCENT;
/// Ordinary text is a paler green; dim text (keys, hints, idle controls) a
/// darker one.
pub const TEXT: Color = Color::srgb_u8(0x9f, 0xf5, 0xc0);
pub const MUTED: Color = Color::srgb_u8(0x4f, 0x8f, 0x66);
pub const ERROR: Color = Color::srgb_u8(0xff, 0x6b, 0x6b);

/// Body text is 13 px, the dumps and tables 12 px, small print 11 px; the
/// titles are body size in bold.
pub const BODY_PX: f32 = 13.0;
pub const MONO_PX: f32 = 12.0;
pub const SMALL_PX: f32 = 11.0;
pub const TITLE_PX: f32 = 13.0;

// --- Surfaces ---------------------------------------------------------------

/// Panel background: near-black at 90 % opacity.
pub const HUD_BG: Color = Color::srgba_u8(0x03, 0x06, 0x05, 230);
/// Nearly opaque panel background: the details card, the help card,
/// tooltips and the selected host's label.
pub const HUD_BG_SOLID: Color = Color::srgba_u8(0x03, 0x06, 0x05, 242);
/// Panel and button borders: the phosphor at 42 % opacity; 75 % under the
/// pointer; 30 % for the rule under a header band.
pub const HUD_BORDER: Color = Color::srgba_u8(0x39, 0xff, 0x88, 107);
pub const HUD_BORDER_HOVER: Color = Color::srgba_u8(0x39, 0xff, 0x88, 191);
pub const HUD_BORDER_SOFT: Color = Color::srgba_u8(0x39, 0xff, 0x88, 77);
/// A header band: the phosphor at 7 % opacity.
pub const BAND: Color = Color::srgba_u8(0x39, 0xff, 0x88, 18);
/// A row or button under the pointer: the phosphor at 8 % opacity.
pub const ROW_HOVER: Color = Color::srgba_u8(0x39, 0xff, 0x88, 20);
/// The selected host's row: the phosphor at 14 %, outlined at 50 %.
pub const ROW_SELECTED: Color = Color::srgba_u8(0x39, 0xff, 0x88, 36);
pub const ROW_SELECTED_BORDER: Color = Color::srgba_u8(0x39, 0xff, 0x88, 128);
/// Text boxes: the panel background.
pub const INPUT_BG: Color = HUD_BG;
/// The help overlay's backdrop: black at 60 % opacity.
pub const BACKDROP: Color = Color::srgba_u8(0x00, 0x00, 0x00, 153);
/// The phosphor at 12 %, 22 % and 18 %: toggle and Scan fills, glows. The
/// glows are kept faint on purpose: a hint of light off the glass, not a
/// bloom.
pub const ACCENT_FILL: Color = Color::srgba_u8(0x39, 0xff, 0x88, 31);
pub const ACCENT_FILL_HOVER: Color = Color::srgba_u8(0x39, 0xff, 0x88, 56);
pub const ACCENT_GLOW: Color = Color::srgba_u8(0x39, 0xff, 0x88, 46);
/// A panel's glow: the phosphor at 4 %.
pub const PANEL_GLOW_COLOR: Color = Color::srgba_u8(0x39, 0xff, 0x88, 10);
/// Stop, while a scan runs: the error colour's family.
pub const STOP: Color = ERROR;
pub const STOP_FILL: Color = Color::srgba_u8(0xff, 0x6b, 0x6b, 31);
pub const STOP_FILL_HOVER: Color = Color::srgba_u8(0xff, 0x6b, 0x6b, 56);
pub const STOP_GLOW: Color = Color::srgba_u8(0xff, 0x6b, 0x6b, 46);

/// Panels and cards sit this far from the window edges.
pub const EDGE: f32 = 10.0;
/// Hard corners everywhere.
pub const PANEL_RADIUS: f32 = 0.0;
pub const CARD_RADIUS: f32 = 0.0;
pub const BUTTON_RADIUS: f32 = 0.0;
/// A panel's glow reaches this far outside it.
pub const PANEL_GLOW: f32 = 8.0;
/// A toggle's glow reaches this far under the pointer; its text has this
/// much air either side.
pub const CHIP_GLOW: f32 = 4.0;
pub const CHIP_PAD_X: f32 = 8.0;
/// The scanlines: every third row of the window is darkened this much.
pub const SCANLINE_STRENGTH: f32 = 0.12;

// --- Looks ------------------------------------------------------------------

/// A floating panel: near-black, a phosphor hairline, a faint glow.
pub fn panel() -> Look {
    Look::bordered(HUD_BG, HUD_BORDER, PANEL_RADIUS).with_glow(PANEL_GLOW_COLOR, PANEL_GLOW)
}

/// A card inside a panel (or a tooltip), nearly opaque, no glow.
pub fn card() -> Look {
    Look::bordered(HUD_BG_SOLID, HUD_BORDER, CARD_RADIUS)
}

/// A header band across the top of a panel.
pub fn band() -> Look {
    Look::filled(BAND, 0.0)
}

/// A toggle that is on: `[x]`, lit, a faint fill, outlined.
pub fn chip_on() -> Look {
    Look::bordered(ACCENT_FILL, HUD_BORDER, BUTTON_RADIUS)
}

pub fn chip_on_hover() -> Look {
    Look::bordered(ACCENT_FILL_HOVER, ACCENT, BUTTON_RADIUS).with_glow(ACCENT_GLOW, CHIP_GLOW)
}

/// A toggle that is off: `[ ]`, dim, nothing around it.
pub fn chip_off() -> Look {
    Look::filled(Color::NONE, BUTTON_RADIUS)
}

pub fn chip_off_hover() -> Look {
    Look::bordered(ROW_HOVER, HUD_BORDER, BUTTON_RADIUS)
}
