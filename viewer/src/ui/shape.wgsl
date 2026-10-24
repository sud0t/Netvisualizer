// The shape shader: every box the UI paints (panels, buttons, toggles,
// swatches, key caps) is one quad drawn with this fragment shader. The
// quad is `size + 2 * pad` pixels; the shape inside it is a rectangle of
// `size` with corners of `radius` (0 here: the terminal has hard corners),
// a border `border` pixels wide inside its edge and a soft glow reaching
// `glow_radius` pixels outside it. A signed distance field gives crisp,
// resolution-independent edges at any size. Last, when `scanline` is set,
// every third row of the window is darkened by that much: a box over the
// whole window with nothing else set is the CRT's scanlines.

// What Bevy's 2D mesh vertex shader hands the fragment stage (only the
// fields read here; the layout matches bevy_sprite_render's VertexOutput).
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

// Mirrors `Shape` in shape.rs: same fields, same order.
struct Shape {
    fill: vec4<f32>,
    stroke: vec4<f32>,
    glow: vec4<f32>,
    size: vec2<f32>,
    pad: f32,
    radius: f32,
    border: f32,
    glow_radius: f32,
    scanline: f32,
};

@group(2) @binding(0) var<uniform> shape: Shape;

// Half a pixel of softness either side of an edge.
const AA: f32 = 0.7;
// A scanline every this many physical pixels.
const SCANLINE_PERIOD: u32 = 3u;

// Signed distance from `p` to the edge of a box of half-size `half` with
// corners of radius `r` (negative inside).
fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(rr);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

// `top` painted over `under`, both with straight (not premultiplied) alpha.
fn over(under: vec4<f32>, top: vec4<f32>) -> vec4<f32> {
    let a = top.a + under.a * (1.0 - top.a);
    if a <= 0.0 {
        return vec4<f32>(0.0);
    }
    let rgb = (top.rgb * top.a + under.rgb * under.a * (1.0 - top.a)) / a;
    return vec4<f32>(rgb, a);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let quad = shape.size + vec2<f32>(2.0 * shape.pad);
    // Pixels from the centre; uv runs from (0, 0) top-left to (1, 1).
    let p = (in.uv - vec2<f32>(0.5)) * quad;
    let half = shape.size * 0.5;
    let d = sd_round_box(p, half, shape.radius);

    var color = vec4<f32>(0.0);

    // The glow: outside the edge, fading quadratically over glow_radius.
    if shape.glow_radius > 0.0 && d > 0.0 {
        let t = 1.0 - clamp(d / shape.glow_radius, 0.0, 1.0);
        color = over(color, vec4<f32>(shape.glow.rgb, shape.glow.a * t * t));
    }

    // The fill, up to the edge.
    let inside = 1.0 - smoothstep(-AA, AA, d);
    color = over(color, vec4<f32>(shape.fill.rgb, shape.fill.a * inside));

    // The border: a band `border` wide just inside the edge.
    if shape.border > 0.0 {
        let band = inside * smoothstep(-shape.border - AA, -shape.border + AA, d);
        color = over(color, vec4<f32>(shape.stroke.rgb, shape.stroke.a * band));
    }

    // The scanlines: `position` is the fragment's place in the window, in
    // physical pixels, so the rows are the window's, not the box's.
    if shape.scanline > 0.0 && (u32(in.position.y) % SCANLINE_PERIOD) == 0u {
        color = over(color, vec4<f32>(0.0, 0.0, 0.0, shape.scanline));
    }

    return color;
}
