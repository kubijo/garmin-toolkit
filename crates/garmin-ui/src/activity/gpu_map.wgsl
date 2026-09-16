struct CameraUniform {
    // xy: high center, zw: low center in normalized Web Mercator coordinates.
    center_high_low: vec4<f32>,
    // xy: viewport size in points, z: pixels per normalized coordinate.
    viewport_world_size_padding: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct TileUniform {
    // xy: high tile origin, zw: low tile origin.
    origin_high_low: vec4<f32>,
    // x: normalized Web Mercator distance represented by one source point.
    normalized_point_scale_padding: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> tile: TileUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) sample_index: f32,
};

struct RouteSegment {
    start: vec2<f32>,
    end: vec2<f32>,
    speed: vec2<f32>,
    sample_indices: vec2<f32>,
};

struct RouteSourceUniform {
    // xy: high route origin, zw: low route origin.
    origin_high_low: vec4<f32>,
};

@group(1) @binding(0)
var<storage, read> route_segments: array<RouteSegment>;

@group(1) @binding(1)
var<uniform> route_source: RouteSourceUniform;

struct RouteStyleUniform {
    // x: stroke width, y: opacity, z: 0/2 for outline or 1/3 for speed colouring.
    // Modes 2 and 3 discard fragments outside index_range_padding.xy.
    width_opacity_mode_padding: vec4<f32>,
    fallback: vec4<f32>,
    index_range_padding: vec4<f32>,
    speed_colors: array<vec4<f32>, 5>,
};

@group(2) @binding(0)
var<uniform> route_style: RouteStyleUniform;

fn viewport() -> vec2<f32> {
    return camera.viewport_world_size_padding.xy;
}

fn world_size() -> f32 {
    return camera.viewport_world_size_padding.z;
}

fn wrapped_delta(origin_high_low: vec4<f32>) -> vec2<f32> {
    // Subtract the split values before adding. Recombining each f64 value first
    // discards the low component at street-level zooms and visibly deforms routes.
    var delta = (origin_high_low.xy - camera.center_high_low.xy)
        + (origin_high_low.zw - camera.center_high_low.zw);
    delta.x = delta.x - round(delta.x);
    return delta;
}

fn clip_position(point: vec2<f32>) -> vec4<f32> {
    let normalized = point / viewport();
    return vec4<f32>(
        normalized.x * 2.0 - 1.0,
        1.0 - normalized.y * 2.0,
        0.0,
        1.0,
    );
}

@vertex
fn vertex_main(
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> VertexOutput {
    let local = position * tile.normalized_point_scale_padding.x;
    let point = (wrapped_delta(tile.origin_high_low) + local) * world_size() + viewport() * 0.5;

    var output: VertexOutput;
    output.position = clip_position(point);
    output.color = color;
    output.sample_index = -1.0;
    return output;
}

fn route_speed_color(speed: f32) -> vec3<f32> {
    let position = clamp(speed, 0.0, 1.0) * 4.0;
    let first = u32(floor(position));
    let second = min(first + 1u, 4u);
    return mix(
        route_style.speed_colors[first].rgb,
        route_style.speed_colors[second].rgb,
        fract(position),
    );
}

@vertex
fn route_vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let segment = route_segments[instance_index];
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_index];
    let origin_pixels = wrapped_delta(route_source.origin_high_low) * world_size() + viewport() * 0.5;
    let start = segment.start * world_size() + origin_pixels;
    let end = segment.end * world_size() + origin_pixels;
    let direction = end - start;
    let length = max(length(direction), 0.0001);
    let normal = vec2<f32>(-direction.y, direction.x) / length;
    let width = route_style.width_opacity_mode_padding.x;
    let point = mix(start, end, corner.x) + normal * corner.y * width * 0.5;

    var color = route_style.fallback;
    let mode = route_style.width_opacity_mode_padding.z;
    if mode == 1.0 || mode == 3.0 {
        let speed = mix(segment.speed.x, segment.speed.y, corner.x);
        if speed >= 0.0 {
            color = vec4<f32>(route_speed_color(speed), 1.0);
        }
    }
    let opacity = route_style.width_opacity_mode_padding.y;

    var output: VertexOutput;
    output.position = clip_position(point);
    output.color = vec4<f32>(color.rgb * opacity, color.a * opacity);
    output.sample_index = mix(segment.sample_indices.x, segment.sample_indices.y, corner.x);
    return output;
}

fn linear_from_gamma_rgb(color: vec3<f32>) -> vec3<f32> {
    let cutoff = color < vec3<f32>(0.04045);
    let lower = color / vec3<f32>(12.92);
    let higher = pow((color + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

@fragment
fn fragment_gamma(output: VertexOutput) -> @location(0) vec4<f32> {
    return output.color;
}

@fragment
fn fragment_linear(output: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(linear_from_gamma_rgb(output.color.rgb), output.color.a);
}

fn route_fragment_color(output: VertexOutput) -> vec4<f32> {
    if route_style.width_opacity_mode_padding.z >= 2.0
        && (output.sample_index < route_style.index_range_padding.x
            || output.sample_index > route_style.index_range_padding.y) {
        discard;
    }
    return output.color;
}

@fragment
fn route_fragment_gamma(output: VertexOutput) -> @location(0) vec4<f32> {
    return route_fragment_color(output);
}

@fragment
fn route_fragment_linear(output: VertexOutput) -> @location(0) vec4<f32> {
    let color = route_fragment_color(output);
    return vec4<f32>(linear_from_gamma_rgb(color.rgb), color.a);
}
