//! Colours, declared once. This table mirrors the custom properties at the top
//! of `pynmapper/graphics/web/style.css`; when one changes, change the other.
//! (theme.js reads them from the stylesheet at runtime; here they are constants.)
#![allow(dead_code)] // the text and HUD colours are used from step 7.2 (labels) and 7.3 (sidebar) on

use bevy::prelude::Color;

pub const SCENE_BG: Color = Color::srgb_u8(0x0b, 0x0f, 0x17);
pub const GRID_LINE: Color = Color::srgb_u8(0x23, 0x33, 0x50);
pub const GRID_AXIS: Color = Color::srgb_u8(0x3a, 0x52, 0x80);
pub const LINK: Color = Color::srgb_u8(0x3d, 0x5a, 0x85);
pub const SELECTION: Color = Color::WHITE;

pub const ROLE_SCANNER: Color = Color::srgb_u8(0x22, 0xd3, 0xee);
pub const ROLE_GATEWAY: Color = Color::srgb_u8(0xf5, 0x9e, 0x0b);
pub const ROLE_HOST: Color = Color::srgb_u8(0x60, 0xa5, 0xfa);
pub const ROLE_DOWN: Color = Color::srgb_u8(0x6b, 0x72, 0x80);

pub const TEXT: Color = Color::srgb_u8(0xd7, 0xdd, 0xe5);
pub const MUTED: Color = Color::srgb_u8(0x8b, 0x96, 0xa5);
pub const ERROR: Color = Color::srgb_u8(0xf8, 0x71, 0x71);
pub const ACCENT: Color = Color::srgb_u8(0x22, 0xd3, 0xee);

/// `--hud-bg`: the scene background at 72 % opacity.
pub const HUD_BG: Color = Color::srgba_u8(0x0b, 0x0f, 0x17, 184);
/// `--hud-bg-solid`
pub const HUD_BG_SOLID: Color = Color::srgba_u8(0x0d, 0x12, 0x1c, 240);
/// `--hud-border`: white at 8 % opacity.
pub const HUD_BORDER: Color = Color::srgba_u8(0xff, 0xff, 0xff, 20);
/// `--row-hover`: white at 5 % opacity.
pub const ROW_HOVER: Color = Color::srgba_u8(0xff, 0xff, 0xff, 13);
/// `--row-selected`: the accent at 14 % opacity.
pub const ROW_SELECTED: Color = Color::srgba_u8(0x22, 0xd3, 0xee, 36);
/// Text boxes: white at 6 % opacity (`input[type="search"]`).
pub const INPUT_BG: Color = Color::srgba_u8(0xff, 0xff, 0xff, 15);
/// The help overlay's backdrop: black at 55 % opacity.
pub const BACKDROP: Color = Color::srgba_u8(0x00, 0x00, 0x00, 140);

/// Opacity of the grid floor (`uOpacity` in the floor shader).
pub const GRID_OPACITY: f32 = 0.6;
/// Opacity of link lines (`LineBasicMaterial.opacity` in graph.js).
pub const LINK_OPACITY: f32 = 0.6;
