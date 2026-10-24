//! Camera, lights, fog, the grid floor and framing.
//! Nothing here knows about hosts; graph.rs puts the network into the world
//! and this module only reads its bounds.

use bevy::color::Alpha;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{Atmosphere, SunDisk};
use bevy::pbr::AtmosphereSettings;
use bevy::prelude::*;
use bevy::render::view::{DebandDither, Tonemapping};
use bevy::window::{PrimaryWindow, WindowResized};

use crate::ViewerSet;
use crate::controls::FlyControls;
use crate::graph::{Bounds, Graph};
use crate::panel::Panel;
use crate::theme;

/// Where the camera looks in from: elevated, from +z. Normalised where used.
pub const DEFAULT_VIEW_DIR: Vec3 = Vec3::new(0.0, 0.55, 1.0);
/// World units; keeps a one-host map from filling the screen.
const MIN_FRAME_SIZE: f32 = 12.0;
/// A little air around the fitted box.
const FRAME_MARGIN: f32 = 1.08;
/// World units per grid cell; major lines every `MAJOR_EVERY` cells.
const GRID_CELL: f32 = 5.0;
const MAJOR_EVERY: i64 = 5;
/// The grid stops growing with the fog here: 201 lines each way, 33
/// faded segments each, is already plenty of gizmo work per frame.
const GRID_LINES_EACH_WAY_MAX: i64 = 100;
/// The grid floats a little below the lowest ring.
const FLOOR_Y: f32 = -3.0;
/// Vertices per grid line, so the distance fade can vary along the line.
const GRID_SEGMENTS: u32 = 32;
const FOV_DEGREES: f32 = 60.0;

#[derive(Component)]
pub struct MainCamera;

/// Fog distances in world units. The grid's lines and the labels fade out
/// over the same range.
#[derive(Resource, Debug, Clone, Copy)]
pub struct SceneSettings {
    pub fog_near: f32,
    pub fog_far: f32,
}

impl Default for SceneSettings {
    fn default() -> Self {
        SceneSettings {
            fog_near: 60.0,
            fog_far: 160.0,
        }
    }
}

/// Pixels per edge hidden under the egui panels.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inset {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// Where the camera should be to show a box, plus the fog that goes with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    pub position: Vec3,
    pub target: Vec3,
    pub fog_near: f32,
    pub fog_far: f32,
    pub far: f32,
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(theme::SCENE_BG))
            // A pale sky-blue ambient light, so the sides of the spheres that
            // face away from the sun are lit too.
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb_u8(0xbf, 0xd4, 0xff),
                brightness: 400.0,
                ..default()
            })
            .init_resource::<SceneSettings>()
            .init_resource::<ViewState>()
            .add_message::<ResetView>()
            .add_systems(Startup, (setup, configure_gizmos))
            .add_systems(
                Update,
                (
                    frame_when_needed.in_set(ViewerSet::Frame),
                    draw_grid.in_set(ViewerSet::Draw),
                ),
            );
    }
}

fn setup(mut commands: Commands, mut mediums: ResMut<Assets<ScatteringMedium>>) {
    // The sky: Bevy's own atmospheric scattering, the sunlight scattered by
    // a planet's air worked out on the GPU (what the bevy_atmosphere crate
    // did for older Bevys). One entity is the planet, Earth's radii and air;
    // its hook puts it 6360 km straight down, so the map lies on the ground
    // under the sky. The camera asks for the sky with `AtmosphereSettings`
    // (which brings `Hdr`), and the sun below gives it its light and colour.
    // It needs compute shaders: on a GPU without them the plugin stays off
    // and the clear colour shows instead. Below the horizon the model draws
    // the planet's surface, lit by the sun through the air; Earth's albedo
    // of 0.3 makes that a beige floor, so the ground is given the scene
    // background's own (near black) colour instead and the grid keeps the
    // dark floor it was drawn for.
    let ground = theme::SCENE_BG.to_linear();
    commands.spawn(Atmosphere {
        ground_albedo: Vec3::new(ground.red, ground.green, ground.blue),
        ..Atmosphere::earth(mediums.add(ScatteringMedium::earth(256, 256)))
    });
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        AtmosphereSettings::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: FOV_DEGREES.to_radians(),
            near: 0.1,
            far: 2000.0,
            ..default()
        }),
        // No tone mapping, so the role colours on the spheres come out as the
        // swatches in the legend and the host list. Debanding only works after
        // tone mapping, so it is off too (Bevy warns about the pair otherwise).
        Tonemapping::None,
        DebandDither::Disabled,
        Transform::from_xyz(0.0, 20.0, 38.0).looking_at(Vec3::ZERO, Vec3::Y),
        DistanceFog {
            color: theme::SCENE_BG,
            directional_light_color: Color::NONE,
            directional_light_exponent: 8.0,
            falloff: FogFalloff::Linear {
                start: 60.0,
                end: 160.0,
            },
        },
    ));

    // The sun, from above and to one side; 6000 lux was tuned by eye on the
    // demo map. It lights the sky too, and shows in it as a disc.
    commands.spawn((
        DirectionalLight {
            color: Color::WHITE,
            illuminance: 6_000.0,
            ..default()
        },
        SunDisk::EARTH,
        Transform::from_xyz(14.0, 30.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.width = 1.5; // pixels: thin grid and link lines
}

/// Whether a map has been framed yet: the first map is framed instantly;
/// later maps glide to a new framing only while the user has not taken the
/// camera themselves.
#[derive(Resource, Debug, Default)]
pub struct ViewState {
    pub framed: bool,
}

/// Ask for the map to be re-framed. R sends it with `animate: true`.
#[derive(Message, Debug, Clone, Copy)]
pub struct ResetView {
    pub animate: bool,
}

/// Frame the map when it changes, when the window is resized, or on request,
/// fitting it into the part of the window the sidebar leaves free.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn frame_when_needed(
    graph: Res<Graph>,
    panel: Res<Panel>,
    mut resized: MessageReader<WindowResized>,
    mut requests: MessageReader<ResetView>,
    mut view: ResMut<ViewState>,
    mut controls: ResMut<FlyControls>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&mut Transform, &mut Projection, &mut DistanceFog), With<MainCamera>>,
    mut settings: ResMut<SceneSettings>,
) {
    let was_resized = !resized.is_empty();
    resized.clear();

    // What kind of framing this frame needs, if any.
    let mut animate: Option<bool> = None;
    if graph.is_changed() && graph.drawn {
        if !view.framed {
            animate = Some(false);
            view.framed = true;
        } else if !controls.user_moved {
            animate = Some(true); // hosts arrived while the view was still the default: keep them all in shot
        }
    }
    if was_resized && !controls.user_moved {
        animate = Some(false);
    }
    if let Some(request) = requests.read().last() {
        animate = Some(request.animate);
        // The view is the program's again, so hosts that arrive later are
        // framed again too, until the user flies off once more.
        controls.user_moved = false;
    }
    let Some(animate) = animate else {
        return;
    };

    let (mut transform, mut projection, mut fog) = camera.into_inner();
    let fov = match &*projection {
        Projection::Perspective(p) => p.fov,
        _ => FOV_DEGREES.to_radians(),
    };
    let viewport = Vec2::new(window.width(), window.height());
    let framing = frame_camera(
        &graph.bounds,
        viewport,
        fov,
        panel.frame_inset(window.width()),
        DEFAULT_VIEW_DIR,
    );
    apply_framing(
        &framing,
        animate,
        &mut transform,
        &mut projection,
        &mut fog,
        &mut settings,
        &mut controls,
    );
}

/// Put the camera where a [`Framing`] says, instantly or as a glide. Fog, far
/// plane and grid fade change at once either way.
pub fn apply_framing(
    framing: &Framing,
    animate: bool,
    transform: &mut Transform,
    projection: &mut Projection,
    fog: &mut DistanceFog,
    settings: &mut SceneSettings,
    controls: &mut FlyControls,
) {
    fog.falloff = FogFalloff::Linear {
        start: framing.fog_near,
        end: framing.fog_far,
    };
    if let Projection::Perspective(p) = projection {
        p.far = framing.far;
    }
    settings.fog_near = framing.fog_near;
    settings.fog_far = framing.fog_far;

    let pose = Transform::from_translation(framing.position).looking_at(framing.target, Vec3::Y);
    if animate {
        controls.tween_to(transform, pose.translation, pose.rotation);
    } else {
        controls.tween = None;
        *transform = pose;
        controls.sync_from_camera(transform);
    }
}

/// Move the camera back along `direction` until every corner of `bounds` is
/// inside the view frustum. Fitting the box rather than its bounding sphere
/// matters because a LAN laid out on rings is a flat disc: seen from above it
/// needs far less distance than a sphere would.
/// `inset` excludes parts of the viewport hidden under panels, so the map is
/// fitted and centred in what is visible. Fog starts just past the far side of
/// the map so the map itself stays crisp and only the grid fades out.
pub fn frame_camera(
    bounds: &Bounds,
    viewport: Vec2,
    fov: f32,
    inset: Inset,
    direction: Vec3,
) -> Framing {
    let (center, mut size) = if bounds.is_empty() {
        (Vec3::ZERO, Vec3::ZERO)
    } else {
        (bounds.center(), bounds.size())
    };
    size.x = size.x.max(MIN_FRAME_SIZE);
    size.y = size.y.max(MIN_FRAME_SIZE / 2.0);
    size.z = size.z.max(MIN_FRAME_SIZE);

    let width = viewport.x.max(1.0);
    let height = viewport.y.max(1.0);

    // The usable region in pixels, measured from the left and from the bottom.
    // Absurd insets (a sidebar wider than the window) are clamped so at least
    // 30 % of each axis is always used.
    let (x0, x1) = usable_span(inset.left, width - inset.right, width);
    let (y0, y1) = usable_span(inset.bottom, height - inset.top, height);

    // View angles (as tangents) of the four edges of the usable region. The
    // vertical fov spans `height` pixels, which gives the tangent per pixel.
    let tan_per_px = 2.0 * (fov / 2.0).tan() / height;
    let tan_left = (width / 2.0 - x0) * tan_per_px;
    let tan_right = (x1 - width / 2.0) * tan_per_px;
    let tan_bottom = (height / 2.0 - y0) * tan_per_px;
    let tan_top = (y1 - height / 2.0) * tan_per_px;
    // Half-angles of the usable region, and how far its centre sits off the view axis.
    let tan_half_w = f32::midpoint(tan_left, tan_right);
    let tan_half_h = f32::midpoint(tan_top, tan_bottom);
    let shift_right = (tan_left - tan_right) / 2.0; // > 0 when the usable centre is left of the viewport centre
    let shift_up = (tan_bottom - tan_top) / 2.0; // > 0 when the usable centre is below it

    // Camera basis for a camera sitting on `direction` and looking at `center`.
    let direction = direction.normalize();
    let forward = -direction;
    let mut right = forward.cross(Vec3::Y);
    if right.length_squared() < 1e-9 {
        right = Vec3::X; // looking straight down
    }
    let right = right.normalize();
    let up = right.cross(forward).normalize();

    // The camera looks at a point `distance × shift` to the side of the box
    // centre, which keeps that centre in the middle of the usable region at
    // any distance. A corner is visible when, seen from the camera, it lies
    // between the left and right edges (and the bottom and top edges) of that
    // region; each edge gives a lower bound on the distance.
    // (Sliding the target by a pixel offset measured at the box centre's depth
    // is simpler, but lets near corners poke out under a wide sidebar.)
    let mut distance: f32 = 0.0;
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 != 0 { 0.5 } else { -0.5 } * size.x,
            if i & 2 != 0 { 0.5 } else { -0.5 } * size.y,
            if i & 4 != 0 { 0.5 } else { -0.5 } * size.z,
        );
        let toward_camera = corner.dot(direction);
        let sideways = corner.dot(right);
        let upward = corner.dot(up);
        distance = distance
            .max((tan_left * toward_camera - sideways) / tan_half_w)
            .max((sideways + tan_right * toward_camera) / tan_half_w)
            .max((tan_bottom * toward_camera - upward) / tan_half_h)
            .max((upward + tan_top * toward_camera) / tan_half_h);
    }
    distance *= FRAME_MARGIN;

    let target = center + right * (distance * shift_right) + up * (distance * shift_up);
    let position = target + direction * distance;

    let radius = size.length() / 2.0;
    let fog_near = distance + radius;
    let fog_far = fog_near + (3.0 * radius).max(40.0);
    Framing {
        position,
        target,
        fog_near,
        fog_far,
        far: 2000f32.max(fog_far * 2.0),
    }
}

/// `[lo, hi]`, widened around its centre (and kept inside `[0, full]`) when it
/// is narrower than 30 % of `full`.
fn usable_span(lo: f32, hi: f32, full: f32) -> (f32, f32) {
    let min_width = full * 0.3;
    if hi - lo >= min_width {
        return (lo, hi);
    }
    let center = f32::midpoint(lo, hi).clamp(min_width / 2.0, full - min_width / 2.0);
    (center - min_width / 2.0, center + min_width / 2.0)
}

/// The floor: an "infinite" grid that follows the camera, drawn as gizmo line
/// strips whose alpha fades with distance like the fog. Minor lines every
/// cell, brighter major lines every five, the two axes in their own colour.
/// (Gizmo lines are drawn as quads, so a line that crosses the near plane
/// still draws; some GL implementations drop raw line primitives there.)
fn draw_grid(
    settings: Res<SceneSettings>,
    camera: Single<&Transform, With<MainCamera>>,
    mut gizmos: Gizmos,
) {
    let eye = camera.translation;
    let (near, far) = (settings.fog_near, settings.fog_far);
    let major = GRID_CELL * MAJOR_EVERY as f32;
    // Snap the grid's centre to a major cell so lines stay put as the camera moves.
    let origin = Vec2::new(
        (eye.x / major).floor() * major,
        (eye.z / major).floor() * major,
    );
    // Lines each way from the centre: as far as the fog reaches, but no
    // more than a map spread far and wide (which pushes the fog out) can
    // afford to draw every frame.
    let count = ((far / GRID_CELL).ceil() as i64).min(GRID_LINES_EACH_WAY_MAX);
    let height_above_floor = (eye.y - FLOOR_Y).abs();

    let fade = |point: Vec3| 1.0 - smoothstep(near, far, eye.distance(point));
    let style = |coordinate: f32| -> (Color, f32) {
        let cells = (coordinate / GRID_CELL).round() as i64;
        if cells == 0 {
            (theme::GRID_AXIS, theme::GRID_OPACITY)
        } else if cells % MAJOR_EVERY == 0 {
            (theme::GRID_LINE, theme::GRID_OPACITY)
        } else {
            (theme::GRID_LINE, theme::GRID_OPACITY * 0.45)
        }
    };

    for i in -count..=count {
        let offset = i as f32 * GRID_CELL;

        // Lines of constant x, running along z.
        let x = origin.x + offset;
        if (eye.x - x).hypot(height_above_floor) < far {
            let (color, alpha) = style(x);
            gizmos.linestrip_gradient((0..=GRID_SEGMENTS).map(|k| {
                let t = -far + 2.0 * far * k as f32 / GRID_SEGMENTS as f32;
                let point = Vec3::new(x, FLOOR_Y, origin.y + t);
                (point, color.with_alpha(alpha * fade(point)))
            }));
        }

        // Lines of constant z, running along x.
        let z = origin.y + offset;
        if (eye.z - z).hypot(height_above_floor) < far {
            let (color, alpha) = style(z);
            gizmos.linestrip_gradient((0..=GRID_SEGMENTS).map(|k| {
                let t = -far + 2.0 * far * k as f32 / GRID_SEGMENTS as f32;
                let point = Vec3::new(origin.x + t, FLOOR_Y, z);
                (point, color.with_alpha(alpha * fade(point)))
            }));
        }
    }
}

/// The usual smoothstep: 0 below `edge0`, 1 above `edge1`, an S-curve between.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Vec2 = Vec2::new(1280.0, 720.0);

    fn corners(bounds: &Bounds) -> Vec<Vec3> {
        (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 != 0 {
                        bounds.max.x
                    } else {
                        bounds.min.x
                    },
                    if i & 2 != 0 {
                        bounds.max.y
                    } else {
                        bounds.min.y
                    },
                    if i & 4 != 0 {
                        bounds.max.z
                    } else {
                        bounds.min.z
                    },
                )
            })
            .collect()
    }

    /// True when every corner is inside the frustum slice of a camera at
    /// `position` looking at `target`, restricted to the usable region.
    fn all_visible(bounds: &Bounds, position: Vec3, target: Vec3, fov: f32, inset: Inset) -> bool {
        let view = Transform::from_translation(position)
            .looking_at(target, Vec3::Y)
            .to_matrix()
            .inverse();
        let tan_v = (fov / 2.0).tan();
        let (w, h) = (VIEWPORT.x, VIEWPORT.y);
        // The usable region in normalised device coordinates.
        let (ndc_left, ndc_right) = (-1.0 + 2.0 * inset.left / w, 1.0 - 2.0 * inset.right / w);
        let (ndc_bottom, ndc_top) = (-1.0 + 2.0 * inset.bottom / h, 1.0 - 2.0 * inset.top / h);
        corners(bounds).into_iter().all(|corner| {
            let p = view.transform_point3(corner);
            let depth = -p.z;
            if depth <= 0.0 {
                return false;
            }
            let ndc_x = p.x / (depth * tan_v * (w / h));
            let ndc_y = p.y / (depth * tan_v);
            (ndc_left - 1e-3..=ndc_right + 1e-3).contains(&ndc_x)
                && (ndc_bottom - 1e-3..=ndc_top + 1e-3).contains(&ndc_y)
        })
    }

    #[test]
    fn framing_fits_every_corner_and_is_not_loose() {
        let fov = FOV_DEGREES.to_radians();
        let bounds = Bounds {
            min: Vec3::new(-20.0, -1.0, -20.0),
            max: Vec3::new(20.0, 4.0, 20.0),
        };
        let f = frame_camera(&bounds, VIEWPORT, fov, Inset::default(), DEFAULT_VIEW_DIR);
        assert!(all_visible(
            &bounds,
            f.position,
            f.target,
            fov,
            Inset::default()
        ));

        // 15 % closer than the 8 % margin allows must push a corner out of view.
        let direction = (f.position - f.target).normalize();
        let distance = f.position.distance(f.target);
        let closer = f.target + direction * distance * 0.85;
        assert!(!all_visible(
            &bounds,
            closer,
            f.target,
            fov,
            Inset::default()
        ));

        // Fog starts beyond the map and the far plane is never nearer than 2000.
        // (frame_camera never fits a box thinner than MIN_FRAME_SIZE / 2 vertically.)
        let size = bounds.size().max(Vec3::new(
            MIN_FRAME_SIZE,
            MIN_FRAME_SIZE / 2.0,
            MIN_FRAME_SIZE,
        ));
        let radius = size.length() / 2.0;
        assert!((f.fog_near - (distance + radius)).abs() < 1e-3);
        assert!(f.fog_far > f.fog_near + 40.0 - 1e-3);
        assert!(f.far >= 2000.0);
    }

    #[test]
    fn framing_respects_a_sidebar_inset() {
        let fov = FOV_DEGREES.to_radians();
        let bounds = Bounds {
            min: Vec3::new(-14.0, 0.0, -14.0),
            max: Vec3::new(14.0, 3.0, 14.0),
        };
        let inset = Inset {
            right: 300.0,
            ..Inset::default()
        };
        let f = frame_camera(&bounds, VIEWPORT, fov, inset, DEFAULT_VIEW_DIR);
        assert!(all_visible(&bounds, f.position, f.target, fov, inset));
        // The usable region is left of centre, so the look target slides right
        // of the box centre (+x is "right" when looking in from +z).
        assert!(f.target.x > bounds.center().x, "{:?}", f.target);
        // A sidebar costs distance compared with the full viewport.
        let full = frame_camera(&bounds, VIEWPORT, fov, Inset::default(), DEFAULT_VIEW_DIR);
        assert!(f.position.distance(f.target) > full.position.distance(full.target));

        // A HUD across the top pushes the look target up, so the map sits lower.
        let inset = Inset {
            top: 200.0,
            ..Inset::default()
        };
        let f = frame_camera(&bounds, VIEWPORT, fov, inset, DEFAULT_VIEW_DIR);
        assert!(all_visible(&bounds, f.position, f.target, fov, inset));
        assert!(f.target.y > bounds.center().y, "{:?}", f.target);
    }

    #[test]
    fn usable_span_keeps_at_least_thirty_percent() {
        assert_eq!(usable_span(0.0, 1280.0, 1280.0), (0.0, 1280.0));
        assert_eq!(usable_span(0.0, 980.0, 1280.0), (0.0, 980.0));
        // 100 px left of a 1280 px window: widened to 384 px, pinned to the left edge.
        assert_eq!(usable_span(0.0, 100.0, 1280.0), (0.0, 384.0));
        // Insets that overlap entirely still leave a centred 30 %.
        let (lo, hi) = usable_span(700.0, 600.0, 1280.0);
        assert!(
            (hi - lo - 384.0).abs() < 1e-3 && lo > 0.0 && hi < 1280.0,
            "{lo} {hi}"
        );
    }

    #[test]
    fn an_empty_map_still_gets_a_sensible_view() {
        let fov = FOV_DEGREES.to_radians();
        let f = frame_camera(
            &Bounds::EMPTY,
            VIEWPORT,
            fov,
            Inset::default(),
            DEFAULT_VIEW_DIR,
        );
        assert_eq!(f.target, Vec3::ZERO);
        let distance = f.position.length();
        assert!(
            distance > MIN_FRAME_SIZE / 2.0 && distance < 100.0,
            "distance {distance}"
        );
    }

    #[test]
    fn smoothstep_clamps_and_eases() {
        assert_eq!(smoothstep(60.0, 160.0, 10.0), 0.0);
        assert_eq!(smoothstep(60.0, 160.0, 200.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((smoothstep(0.0, 1.0, 0.25) - 0.15625).abs() < 1e-6);
    }
}
