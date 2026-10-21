//! The egui layer over the 3D view. Step 7.2: host labels drawn with the
//! painter (the CSS2D labels of graph.js and `.label` in style.css) and the
//! [`UiInput`] flags that tell controls and picking when egui owns the pointer
//! or keyboard. Step 7.3 adds the HUD, sidebar, legend and help overlay here.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::graph::{ColorKey, Graph, LABEL_GAP, Node};
use crate::picking::Picker;
use crate::scene::{MainCamera, SceneSettings, smoothstep};
use crate::theme;

/// `--mono` is 12 px; `.label` has `padding: 2px 7px; border-radius: 4px`.
const LABEL_FONT_PX: f32 = 12.0;
const LABEL_PADDING: egui::Vec2 = egui::vec2(7.0, 2.0);
const LABEL_ROUNDING: f32 = 4.0;
/// Labels fainter than this are not worth a draw call.
const MIN_LABEL_OPACITY: f32 = 0.01;

/// What egui wants this frame: `isTypingTarget` and "pointer over the
/// sidebar" in the browser, where the DOM sorted this out for free.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UiInput {
    /// The pointer is over an egui panel or egui is dragging something.
    pub pointer: bool,
    /// A text box has focus: keys type instead of flying.
    pub keyboard: bool,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiInput>()
            .add_systems(EguiPrimaryContextPass, (draw_labels, read_ui_input).chain());
    }
}

/// Colours of one label, after the CSS cascade in style.css.
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

/// `fadeLabels`: fully visible up to the fog start, gone at the fog end.
pub fn label_opacity(distance: f32, settings: &SceneSettings) -> f32 {
    1.0 - smoothstep(settings.fog_near, settings.fog_far, distance)
}

/// A Bevy colour as egui sees it, with the whole label's opacity folded in
/// (CSS `opacity` scales background, border and text alike).
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
/// under every panel so the sidebar covers labels the way the DOM did.
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

    for (entity, node, transform) in &nodes {
        let opacity = label_opacity(
            camera_transform.translation.distance(transform.translation),
            &settings,
        );
        if opacity < MIN_LABEL_OPACITY {
            continue;
        }
        // Child of the sphere in graph.js: a fixed gap above the top of the sphere.
        let anchor = transform.translation + Vec3::Y * (node.radius + LABEL_GAP);
        let Ok(screen) = camera.world_to_viewport(&camera_global, anchor) else {
            continue; // behind the camera
        };
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

/// Text in a rounded box whose bottom-centre sits at `anchor`
/// (`CSS2DObject.center = (0.5, 1)`).
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
        egui::FontId::monospace(LABEL_FONT_PX),
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

/// Runs after the UI was built for this frame, so the flags are ready for
/// next frame's controls and picking.
fn read_ui_input(mut contexts: EguiContexts, mut ui: ResMut<UiInput>) -> Result {
    let ctx = contexts.ctx_mut()?;
    let next = UiInput {
        pointer: ctx.egui_wants_pointer_input() || ctx.is_pointer_over_egui(),
        keyboard: ctx.egui_wants_keyboard_input(),
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
}
