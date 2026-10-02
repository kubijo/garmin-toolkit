@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[index], 0.0, 1.0);
}

@fragment
fn transparent() -> @location(0) vec4<f32> {
    return vec4(0.0);
}

@fragment
fn pattern(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let cell = vec2<u32>(position.xy / 32.0);
    let alternate = (cell.x + cell.y) % 2u == 0u;
    return select(vec4(0.08, 0.25, 0.5, 1.0), vec4(0.9, 0.55, 0.08, 1.0), alternate);
}
