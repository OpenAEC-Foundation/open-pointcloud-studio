// Camera layout and perspective projection match points.wgsl.
struct Camera {
    right: vec4<f32>, up: vec4<f32>, toward: vec4<f32>,
    projection: vec4<f32>, view: vec4<f32>, surface: vec4<f32>,
    clip_min: vec4<f32>, clip_max: vec4<f32>, clip_enabled: vec4<f32>, splat: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
struct Vertex { relative: vec4<f32>, color: vec4<f32>, normal: vec4<f32> };
@group(1) @binding(0) var<storage, read> vertices0: array<Vertex>;
@group(1) @binding(1) var<storage, read> vertices1: array<Vertex>;
@group(1) @binding(2) var<storage, read> triangles: array<vec4<u32>>;
@group(1) @binding(3) var<storage, read> charts: array<vec4<u32>>;
@group(1) @binding(4) var atlas: texture_2d_array<f32>;
@group(1) @binding(5) var atlas_sampler: sampler;
@group(1) @binding(6) var<uniform> settings: vec4<f32>;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
    @location(3) depth: f32,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Output {
    let face = index / 3u;
    let corner = index % 3u;
    let vertex = triangles[face][corner];
    var input: Vertex;
    if vertex < 2000000u { input = vertices0[vertex]; }
    else { input = vertices1[vertex - 2000000u]; }
    let chart = charts[face];
    var uv = vec2<f32>(f32(chart.y) + 1.5, f32(chart.z) + 1.5);
    if corner == 1u { uv.x = f32(chart.y + chart.w) + 0.5; }
    if corner == 2u { uv.y = f32(chart.z + chart.w) + 0.5; }
    uv /= settings.x;
    // Two pages per array layer retain all 512 supported atlas pages while
    // staying within the portable 256-array-layer limit.
    uv.x = (uv.x + f32(chart.x % 2u)) * 0.5;
    let depth = camera.toward.w - dot(input.relative.xyz, camera.toward.xyz);
    let scale = camera.projection.z * camera.view.w;
    let offset = (camera.surface.xy + camera.projection.xy * 0.5 + camera.view.xy) * camera.view.w;
    let across = vec2<f32>(
        dot(input.relative.xyz, camera.right.xyz) - camera.right.w,
        camera.up.w - dot(input.relative.xyz, camera.up.xyz)
    ) * scale;
    let physical_w = offset * depth + across;
    var output: Output;
    output.position = vec4<f32>(physical_w.x / camera.surface.z * 2.0 - depth,
        depth - physical_w.y / camera.surface.w * 2.0, depth - 0.01, depth);
    output.relative = input.relative.xyz;
    output.uv = uv;
    output.layer = chart.x / 2u;
    output.depth = depth;
    return output;
}
struct Fragment { @location(0) color: vec4<f32>, @builtin(frag_depth) depth: f32 };
@fragment
fn fs_main(input: Output) -> Fragment {
    if camera.clip_enabled.x >= 0.5 {
        var at = input.relative;
        if camera.clip_enabled.x > 1.5 {
            let center = (camera.clip_min.xyz + camera.clip_max.xyz) * 0.5;
            let offset = at.xy - center.xy;
            at = vec3<f32>(center.x + camera.clip_max.w * offset.x + camera.clip_min.w * offset.y,
                center.y - camera.clip_min.w * offset.x + camera.clip_max.w * offset.y, at.z);
        }
        let reach = max(abs(camera.clip_min.xyz), abs(camera.clip_max.xyz));
        let slack = 1e-5 * max(max(reach.x, reach.y), max(reach.z, 1.0));
        if any(at < camera.clip_min.xyz - vec3<f32>(slack)) || any(at > camera.clip_max.xyz + vec3<f32>(slack)) { discard; }
    }
    var output: Fragment;
    // The scene buffer stores encoded colour bytes, as the point/photo
    // pipelines do. Photographs already include the measured illumination.
    output.color = vec4<f32>(textureSampleLevel(atlas, atlas_sampler, input.uv, i32(input.layer), 0.0).rgb, 1.0);
    output.depth = clamp(input.depth / (camera.projection.w * 4.0), 0.0, 1.0);
    return output;
}
