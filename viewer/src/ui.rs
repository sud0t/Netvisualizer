//! The egui layer over the 3D view: host labels drawn with the painter, the
//! HUD, the legend and the help overlay, the sidebar from panel.rs, and the
//! [`UiInput`] flags that tell controls and picking when egui owns the pointer
//! or keyboard.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::controls::FlyControls;
use crate::graph::{ColorKey, Graph, LABEL_GAP, Node};
use crate::map::Role;
use crate::panel::{card, draw_sidebar};
use crate::picking::Picker;
use crate::scene::{MainCamera, SceneSettings, smoothstep};
use crate::theme;

/// Body text is 14 px, monospace 12 px.
pub const BODY_PX: f32 = 14.0;
pub const MONO_PX: f32 = 12.0;
/// A label's box: 7 px of padding beside the text, 2 px above and below, and
/// 4 px corners.
const LABEL_PADDING: egui::Vec2 = egui::vec2(7.0, 2.0);
const LABEL_ROUNDING: f32 = 4.0;
/// Labels fainter than this are not worth a draw call.
const MIN_LABEL_OPACITY: f32 = 0.01;
/// The HUD and the legend sit 12 px from the window edges.
const EDGE: f32 = 12.0;

/// What egui wants this frame, so the 3D controls and picking stay out of its
/// way.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UiInput {
    /// The pointer is over an egui panel or egui is dragging something.
    pub pointer: bool,
    /// A text box has focus: keys type instead of flying.
    pub keyboard: bool,
}

/// The HUD's status line.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    /// Drawn in the error colour.
    pub error: bool,
}

impl Default for Status {
    fn default() -> Self {
        Status::message("no scan yet · start one with --scan")
    }
}

impl Status {
    /// A plain message, shown until the first map arrives.
    pub fn message(text: &str) -> Status {
        Status {
            text: text.to_string(),
            error: false,
        }
    }

    /// A new map arrived: show its summary.
    pub fn show_map(&mut self, summary: String) {
        self.text = summary;
        self.error = false;
    }

    pub fn show_error(&mut self, message: String) {
        self.text = message;
        self.error = true;
    }
}

/// The help overlay with the controls (`?`, `h`, the HUD button; Escape or a
/// click on the backdrop closes it).
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Help {
    pub open: bool,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiInput>()
            .init_resource::<Status>()
            .init_resource::<Help>()
            .add_systems(
                EguiPrimaryContextPass,
                (
                    apply_style,
                    draw_hud,
                    draw_legend,
                    draw_sidebar,
                    draw_help,
                    draw_labels,
                    pointer_cursor,
                    read_ui_input,
                )
                    .chain(),
            );
    }
}

/// The role colours of the host list's dots and the legend's swatches
/// (unknown roles and down hosts use the host colour there; only the 3D
/// spheres turn grey).
pub fn role_color(role: Role) -> Color {
    match role {
        Role::Scanner => theme::ROLE_SCANNER,
        Role::Gateway => theme::ROLE_GATEWAY,
        Role::Host => theme::ROLE_HOST,
    }
}

/// Body text: proportional, 14 px.
pub fn text(s: &str, color: Color) -> egui::RichText {
    egui::RichText::new(s)
        .size(BODY_PX)
        .color(egui_color(color, 1.0))
}

/// Monospace text, 12 px.
pub fn mono(s: &str, color: Color) -> egui::RichText {
    egui::RichText::new(s)
        .family(egui::FontFamily::Monospace)
        .size(MONO_PX)
        .color(egui_color(color, 1.0))
}

/// The fly speed in the HUD: one decimal below 10 u/s, whole numbers above.
pub fn speed_text(speed: f32) -> String {
    if speed < 10.0 {
        format!("{speed:.1} u/s")
    } else {
        format!("{} u/s", speed.round())
    }
}

/// The viewer's look, set once on the first egui pass: font sizes, button and
/// text box looks, tooltips.
fn apply_style(mut contexts: EguiContexts, mut done: Local<bool>) -> Result {
    if *done {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(viewer_style);
    *done = true;
    Ok(())
}

pub fn viewer_style(style: &mut egui::Style) {
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(BODY_PX, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(BODY_PX, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(16.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(MONO_PX, FontFamily::Monospace),
        ),
    ]
    .into();
    // Text in the HUD and the card is for reading, not for drag-selecting.
    style.interaction.selectable_labels = false;
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    // Keep a gutter for the (floating) scroll bar so it never covers the
    // right-aligned addresses in the host list.
    style.spacing.scroll.floating_allocated_width = 6.0;

    let border = egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0));
    let text_color = egui_color(theme::TEXT, 1.0);
    let visuals = &mut style.visuals;
    visuals.widgets.noninteractive.fg_stroke.color = text_color;
    visuals.widgets.noninteractive.bg_stroke = border;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        // Buttons: a 1 px border in the panel border colour, 6 px corners.
        widget.corner_radius = 6.into();
        widget.bg_stroke = border;
        widget.fg_stroke.color = text_color;
        widget.expansion = 0.0;
    }
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
    visuals.widgets.hovered.weak_bg_fill = egui_color(theme::ROW_HOVER, 1.0);
    visuals.widgets.active.weak_bg_fill = egui_color(theme::ROW_HOVER, 2.0);
    // A focused text box gets an accent border; selected text an accent tint.
    visuals.selection.stroke = egui::Stroke::new(1.0, egui_color(theme::ACCENT, 1.0));
    visuals.selection.bg_fill = egui_color(theme::ACCENT, 0.35);
    visuals.extreme_bg_color = egui_color(theme::INPUT_BG, 1.0);
    visuals.faint_bg_color = egui_color(theme::ROW_HOVER, 1.0); // table stripes
    // Tooltips.
    visuals.window_fill = egui_color(theme::HUD_BG_SOLID, 1.0);
    visuals.window_stroke = border;
    visuals.window_corner_radius = 6.into();
}

/// The HUD: brand, status line, speed, help button. Top left.
fn draw_hud(
    mut contexts: EguiContexts,
    status: Res<Status>,
    controls: Res<FlyControls>,
    mut help: ResMut<Help>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("hud"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(EDGE, EDGE))
        .show(ctx, |ui| {
            card(8)
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 8,
                    top: 6,
                    bottom: 6,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        ui.add(
                            egui::Label::new(text("Netvisualizer", theme::TEXT).strong()).extend(),
                        );
                        let color = if status.error {
                            theme::ERROR
                        } else {
                            theme::MUTED
                        };
                        ui.add(egui::Label::new(mono(&status.text, color)).extend());
                        // A thin rule between the status line and the speed.
                        let (rule, _) =
                            ui.allocate_exact_size(egui::vec2(1.0, 16.0), egui::Sense::hover());
                        ui.painter().vline(
                            rule.center().x,
                            rule.y_range(),
                            egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)),
                        );
                        ui.add(
                            egui::Label::new(mono(&speed_text(controls.speed), theme::MUTED))
                                .extend(),
                        )
                        .on_hover_text("fly speed: mouse wheel changes it");
                        let button = egui::Button::new(text("?", theme::MUTED))
                            .min_size(egui::vec2(26.0, 26.0))
                            .corner_radius(13);
                        if ui.add(button).on_hover_text("controls (?)").clicked() {
                            help.open = !help.open;
                        }
                    });
                });
        });
    Ok(())
}

/// The legend: what the colours mean. Bottom left, ignores the pointer.
fn draw_legend(mut contexts: EguiContexts) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("legend"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(EDGE, -EDGE))
        .interactable(false)
        .show(ctx, |ui| {
            card(8)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (i, role) in [Role::Scanner, Role::Gateway, Role::Host]
                            .into_iter()
                            .enumerate()
                        {
                            if i > 0 {
                                ui.add_space(14.0 - 6.0);
                            }
                            let (swatch, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                swatch.center(),
                                5.0,
                                egui_color(role_color(role), 1.0),
                            );
                            ui.label(mono(role.name(), theme::MUTED));
                        }
                    });
                });
        });
    Ok(())
}

/// What triggers an action in the help table: keys (drawn as key caps) or a mouse gesture.
enum Trigger {
    Keys(&'static [&'static str]),
    Mouse(&'static str),
}

/// The rows of the help table.
const HELP_ROWS: [(Trigger, &str); 12] = [
    (
        Trigger::Keys(&["W", "A", "S", "D"]),
        "fly forward / left / back / right",
    ),
    (Trigger::Keys(&["Q", "E"]), "down / up"),
    (Trigger::Keys(&["Shift"]), "fly faster"),
    (Trigger::Mouse("right mouse (hold)"), "look around"),
    (Trigger::Mouse("mouse wheel"), "change fly speed"),
    (
        Trigger::Mouse("left click"),
        "select a host, open its details",
    ),
    (Trigger::Mouse("double click"), "fly to a host"),
    (Trigger::Keys(&["F"]), "fly to the selected host"),
    (Trigger::Keys(&["R"]), "reset the view"),
    (Trigger::Keys(&["/"]), "filter the host list"),
    (Trigger::Keys(&["Esc"]), "deselect / close"),
    (Trigger::Keys(&["?"]), "this help"),
];

/// A key cap.
fn kbd(ui: &mut egui::Ui, key: &str) {
    egui::Frame::new()
        .fill(egui_color(theme::INPUT_BG, 1.0))
        .stroke(egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(mono(key, theme::TEXT)));
}

/// The help overlay: a card over a dimmed backdrop; a click on the backdrop closes it.
/// (Escape is handled with the other keys in main.rs, so it never also deselects.)
fn draw_help(mut contexts: EguiContexts, mut help: ResMut<Help>) -> Result {
    if !help.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let frame = card(10)
        .fill(egui_color(theme::HUD_BG_SOLID, 1.0))
        .inner_margin(egui::Margin::symmetric(22, 18));
    let response = egui::Modal::new(egui::Id::new("help"))
        .backdrop_color(egui_color(theme::BACKDROP, 1.0))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(text("Controls", theme::TEXT).size(16.0).strong());
            ui.add_space(8.0);
            egui::Grid::new("help-table")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    for (trigger, action) in &HELP_ROWS {
                        match trigger {
                            Trigger::Keys(keys) => {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    for key in *keys {
                                        kbd(ui, key);
                                    }
                                });
                            }
                            Trigger::Mouse(gesture) => {
                                ui.label(text(gesture, theme::MUTED));
                            }
                        }
                        ui.label(text(action, theme::TEXT));
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(text("Press", theme::MUTED).size(MONO_PX));
                kbd(ui, "?");
                ui.label(text("or", theme::MUTED).size(MONO_PX));
                kbd(ui, "Esc");
                ui.label(text("to close.", theme::MUTED).size(MONO_PX));
            });
        });
    if response.backdrop_response.clicked() {
        help.open = false;
    }
    Ok(())
}

/// Colours of one label, from its role, hover and selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelStyle {
    pub text: Color,
    pub background: Color,
    pub border: Color,
}

impl LabelStyle {
    pub fn new(key: ColorKey, hovered: bool, selected: bool) -> LabelStyle {
        let text = match key {
            ColorKey::Scanner => theme::ROLE_SCANNER,
            ColorKey::Gateway => theme::ROLE_GATEWAY,
            ColorKey::Down => theme::ROLE_DOWN,
            ColorKey::Host => theme::TEXT,
        };
        let background = if selected {
            theme::HUD_BG_SOLID
        } else {
            theme::HUD_BG
        };
        let border = if selected {
            theme::SELECTION
        } else if hovered {
            Color::srgba(1.0, 1.0, 1.0, 0.35)
        } else {
            theme::HUD_BORDER
        };
        LabelStyle {
            text,
            background,
            border,
        }
    }
}

/// A label's opacity: fully visible up to the fog start, gone at the fog end.
pub fn label_opacity(distance: f32, settings: &SceneSettings) -> f32 {
    1.0 - smoothstep(settings.fog_near, settings.fog_far, distance)
}

/// A Bevy colour as egui sees it, with the whole label's opacity folded in,
/// so a fading label's background, border and text fade alike.
pub fn egui_color(color: Color, opacity: f32) -> egui::Color32 {
    let c = color.to_srgba();
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
        channel(c.red),
        channel(c.green),
        channel(c.blue),
        channel(c.alpha * opacity),
    )
}

/// One label per host, bottom-centred just above its sphere, on the layer
/// under every panel so the sidebar covers labels. Painted far to near, so
/// the nearer of two overlapping labels is on top.
fn draw_labels(
    mut contexts: EguiContexts,
    camera: Single<(&Camera, &Transform), With<MainCamera>>,
    nodes: Query<(Entity, &Node, &Transform), Without<MainCamera>>,
    graph: Res<Graph>,
    picker: Res<Picker>,
    settings: Res<SceneSettings>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(egui::LayerId::background());
    // One egui point is one logical pixel divided by egui's zoom factor
    // (Ctrl+plus/minus change it), and world_to_viewport answers in logical pixels.
    let points_per_pixel = 1.0 / ctx.zoom_factor();
    let (camera, camera_transform) = camera.into_inner();
    let camera_global = GlobalTransform::from(*camera_transform);
    let selected = picker.selected_entity(&graph);

    let mut labels: Vec<(f32, Entity, &Node, Vec2)> = nodes
        .iter()
        .filter_map(|(entity, node, transform)| {
            let distance = camera_transform.translation.distance(transform.translation);
            // A fixed gap above the top of the sphere.
            let anchor = transform.translation + Vec3::Y * (node.radius + LABEL_GAP);
            let screen = camera.world_to_viewport(&camera_global, anchor).ok()?; // behind the camera
            Some((distance, entity, node, screen))
        })
        .collect();
    labels.sort_by(|a, b| b.0.total_cmp(&a.0));

    for (distance, entity, node, screen) in labels {
        let opacity = label_opacity(distance, &settings);
        if opacity < MIN_LABEL_OPACITY {
            continue;
        }
        let style = LabelStyle::new(
            node.key,
            Some(entity) == picker.hovered,
            Some(entity) == selected,
        );
        let anchor = egui::pos2(screen.x * points_per_pixel, screen.y * points_per_pixel);
        paint_label(&painter, anchor, &node.label, &style, opacity);
    }
    Ok(())
}

/// Text in a rounded box whose bottom-centre sits at `anchor`.
fn paint_label(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    text: &str,
    style: &LabelStyle,
    opacity: f32,
) {
    let text_color = egui_color(style.text, opacity);
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(MONO_PX),
        text_color,
    );
    let size = galley.size() + 2.0 * LABEL_PADDING;
    // Whole pixels keep the 1 px border crisp.
    let min = egui::pos2(
        (anchor.x - size.x / 2.0).round(),
        (anchor.y - size.y).round(),
    );
    let rect = egui::Rect::from_min_size(min, size);
    painter.rect(
        rect,
        LABEL_ROUNDING,
        egui_color(style.background, opacity),
        egui::Stroke::new(1.0, egui_color(style.border, opacity)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + LABEL_PADDING, galley, text_color);
}

/// A pointing-hand cursor while a host is under the pointer. egui owns the
/// window's cursor icon (bevy_egui applies it), so ask egui for it.
fn pointer_cursor(
    mut contexts: EguiContexts,
    picker: Res<Picker>,
    controls: Res<FlyControls>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if picker.hovered.is_some() && !controls.looking && !ctx.is_pointer_over_egui() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    Ok(())
}

/// Runs after the UI was built for this frame, so the flags are ready for
/// next frame's controls and picking. Only a focused text box takes the
/// keyboard: a button focused with Tab does not stop WASD.
fn read_ui_input(mut contexts: EguiContexts, mut ui: ResMut<UiInput>) -> Result {
    let ctx = contexts.ctx_mut()?;
    let next = UiInput {
        pointer: ctx.egui_wants_pointer_input() || ctx.is_pointer_over_egui(),
        keyboard: ctx.text_edit_focused(),
    };
    if *ui != next {
        *ui = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fade_over_the_fog_range() {
        let settings = SceneSettings {
            fog_near: 60.0,
            fog_far: 160.0,
        };
        assert_eq!(label_opacity(10.0, &settings), 1.0);
        assert_eq!(label_opacity(60.0, &settings), 1.0);
        assert!((label_opacity(110.0, &settings) - 0.5).abs() < 1e-6);
        assert_eq!(label_opacity(200.0, &settings), 0.0);
    }

    #[test]
    fn label_styles_follow_the_stylesheet_cascade() {
        let plain = LabelStyle::new(ColorKey::Host, false, false);
        assert_eq!(plain.text, theme::TEXT);
        assert_eq!(plain.background, theme::HUD_BG);
        assert_eq!(plain.border, theme::HUD_BORDER);
        assert_eq!(
            LabelStyle::new(ColorKey::Scanner, false, false).text,
            theme::ROLE_SCANNER
        );
        assert_eq!(
            LabelStyle::new(ColorKey::Gateway, false, false).text,
            theme::ROLE_GATEWAY
        );
        assert_eq!(
            LabelStyle::new(ColorKey::Down, false, false).text,
            theme::ROLE_DOWN
        );
        let hovered = LabelStyle::new(ColorKey::Host, true, false);
        assert_eq!(hovered.border, Color::srgba(1.0, 1.0, 1.0, 0.35));
        assert_eq!(hovered.background, theme::HUD_BG);
        let selected = LabelStyle::new(ColorKey::Host, true, true);
        assert_eq!(
            selected.border,
            theme::SELECTION,
            "selected wins over hovered"
        );
        assert_eq!(selected.background, theme::HUD_BG_SOLID);
    }

    #[test]
    fn opacity_scales_the_alpha_channel_only() {
        // Color32 stores premultiplied alpha, so compare against the same constructor.
        let c = egui_color(theme::HUD_BG, 0.5);
        assert_eq!(
            c,
            egui::Color32::from_rgba_unmultiplied(0x0b, 0x0f, 0x17, 92)
        ); // 184 * 0.5
        assert_eq!(c.a(), 92);
        assert_eq!(egui_color(theme::SELECTION, 1.0), egui::Color32::WHITE);
        assert_eq!(egui_color(theme::SELECTION, 0.0).a(), 0);
    }

    #[test]
    fn speed_text_has_one_decimal_below_ten() {
        assert_eq!(speed_text(18.0), "18 u/s");
        assert_eq!(speed_text(18.5), "19 u/s");
        assert_eq!(speed_text(9.96), "10.0 u/s");
        assert_eq!(speed_text(1.0), "1.0 u/s");
        assert_eq!(speed_text(500.0), "500 u/s");
    }

    #[test]
    fn the_status_line_shows_a_message_a_map_or_an_error() {
        let mut status = Status::default();
        assert_eq!(status.text, "no scan yet · start one with --scan");
        status.show_error("scan failed: boom".into());
        assert!(status.error);
        status.show_map("ping_sweep · done · 3 hosts · rev 4".into());
        assert_eq!(status.text, "ping_sweep · done · 3 hosts · rev 4");
        assert!(!status.error, "a new map clears the error");
    }

    /// Every character the UI draws must exist in egui's bundled fonts, or it
    /// shows up as a box: the viewer ships no fonts of its own. (`has_glyph`
    /// cannot answer this: it reports every glyph of the font that also holds
    /// the replacement glyph as missing, so compare what actually gets drawn.)
    #[test]
    fn the_bundled_fonts_have_every_glyph_the_ui_uses() {
        let ctx = egui::Context::default();
        // No renderer here to upload the font atlas to; drop the upload on purpose.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let drawn_as = |font: &egui::FontId, c: char| {
            ctx.fonts_mut(|fonts| {
                let galley =
                    fonts.layout_no_wrap(c.to_string(), font.clone(), egui::Color32::WHITE);
                galley.rows[0].glyphs[0].uv_rect
            })
        };
        // Text in the HUD, card and labels: "·" and "…" arrive in summaries
        // and stage names from the scan, "—" is an empty value.
        let used = [
            (
                "proportional",
                egui::FontId::proportional(BODY_PX),
                "‹›×?·…—",
            ),
            ("monospace", egui::FontId::monospace(MONO_PX), "·…—/?-"),
        ];
        for (family, font, chars) in used {
            let missing = drawn_as(&font, '\u{10FFFD}'); // private use: never in a font
            for c in chars.chars() {
                assert_ne!(
                    drawn_as(&font, c),
                    missing,
                    "{family} font has no glyph for {c:?}"
                );
            }
            assert_eq!(drawn_as(&font, '✕'), missing, "the check can fail");
        }
    }
}
