// NEO map shader (WGSL, wgpu 30).
//
// Vertex positions are f32 WebMercator metres relative to the scene origin.
// The camera uniform supplies: (center - origin), viewport in physical pixels
// and pixels-per-metre. Pan/zoom therefore costs one 32-byte uniform write,
// not a geometry re-upload.

struct Camera {
    // camera centre relative to the batch origin (metres)
    center: vec2<f32>,
    // viewport size in physical pixels
    viewport: vec2<f32>,
    // pixels per metre
    scale: f32,
    // 0 = normal colour, 1 = fully desaturated (inactive/read-only layer)
    grey: f32,
    _pad0: vec2<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    // metres (relative to origin) -> screen pixels
    let screen = (in.position - camera.center) * camera.scale + camera.viewport * 0.5;

    // screen pixels -> normalised device coordinates (y axis points down on screen)
    let ndc = vec2<f32>(
        screen.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - screen.y / camera.viewport.y * 2.0,
    );

    var out: VertexOut;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Inactive layer: desaturated + 50% opacity (not a gray overlay mask).
    let luma = dot(in.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let rgb = mix(in.color.rgb, vec3<f32>(luma), camera.grey);
    let alpha = in.color.a * mix(1.0, 0.5, camera.grey);
    return vec4<f32>(rgb, alpha);
}
