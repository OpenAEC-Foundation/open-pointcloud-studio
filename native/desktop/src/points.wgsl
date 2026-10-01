struct Camera {
    right: vec4<f32>,
    up: vec4<f32>,
    toward: vec4<f32>,
    projection: vec4<f32>, // local width, height, scale, camera distance
    view: vec4<f32>,       // pan x, pan y, point size, display scale
    surface: vec4<f32>,     // widget x, y, target physical width, height
    clip_min: vec4<f32>,
    clip_max: vec4<f32>,
    clip_enabled: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var scene_color: texture_2d<f32>;
@group(1) @binding(1) var scene_depth: texture_depth_2d;

struct VertexInput {
    @builtin(vertex_index) vertex: u32,
    @location(0) relative: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) relative: vec3<f32>,
    @location(3) normal: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let depth = camera.projection.w - dot(input.relative.xyz, camera.toward.xyz);
    var output: VertexOutput;
    output.color = input.color;
    output.local = vec2<f32>(0.0, 0.0);
    output.relative = input.relative.xyz;
    output.normal = vec3<f32>(0.0, 0.0, 0.0);
    if depth <= 0.01 {
        output.position = vec4<f32>(2.0, 2.0, 1.0, 1.0);
        return output;
    }

    let local_x = camera.projection.x * 0.5 + camera.view.x
        + dot(input.relative.xyz, camera.right.xyz) * camera.projection.z / depth;
    let local_y = camera.projection.y * 0.5 + camera.view.y
        - dot(input.relative.xyz, camera.up.xyz) * camera.projection.z / depth;
    let physical = (camera.surface.xy + vec2<f32>(local_x, local_y)) * camera.view.w;
    let center = vec2<f32>(
        physical.x / camera.surface.z * 2.0 - 1.0,
        1.0 - physical.y / camera.surface.w * 2.0
    );
    let radius = vec2<f32>(
        camera.view.z * camera.view.w / camera.surface.z,
        camera.view.z * camera.view.w / camera.surface.w
    );
    let corner = vec2<f32>(
        select(-1.0, 1.0, input.vertex == 1u || input.vertex >= 4u),
        select(-1.0, 1.0, input.vertex == 2u || input.vertex == 3u || input.vertex == 5u)
    );
    output.local = corner;
    output.position = vec4<f32>(
        center.x + corner.x * radius.x,
        center.y + corner.y * radius.y,
        clamp(depth / (camera.projection.w * 4.0), 0.0, 1.0),
        1.0
    );
    return output;
}

struct MeshInput {
    @location(0) relative: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) normal: vec4<f32>,
};

@vertex
fn vs_mesh(input: MeshInput) -> VertexOutput {
    let depth = camera.projection.w - dot(input.relative.xyz, camera.toward.xyz);
    var output: VertexOutput;
    output.color = input.color;
    output.local = vec2<f32>(0.0, 0.0);
    output.relative = input.relative.xyz;
    output.normal = input.normal.xyz;
    if depth <= 0.01 {
        output.position = vec4<f32>(2.0, 2.0, 1.0, 1.0);
        return output;
    }
    let local_x = camera.projection.x * 0.5 + camera.view.x
        + dot(input.relative.xyz, camera.right.xyz) * camera.projection.z / depth;
    let local_y = camera.projection.y * 0.5 + camera.view.y
        - dot(input.relative.xyz, camera.up.xyz) * camera.projection.z / depth;
    let physical = (camera.surface.xy + vec2<f32>(local_x, local_y)) * camera.view.w;
    output.position = vec4<f32>(
        physical.x / camera.surface.z * 2.0 - 1.0,
        1.0 - physical.y / camera.surface.w * 2.0,
        clamp(depth / (camera.projection.w * 4.0), 0.0, 1.0),
        1.0
    );
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if camera.clip_enabled.x > 0.5 &&
       (any(input.relative < camera.clip_min.xyz) || any(input.relative > camera.clip_max.xyz)) {
        discard;
    }
    if dot(input.normal, input.normal) <= 0.000001 {
        return input.color;
    }
    let light = normalize(vec3<f32>(-0.35, -0.25, 0.90));
    let shade = 0.76 + 0.24 * max(dot(normalize(input.normal), light), 0.0);
    return vec4<f32>(input.color.rgb * shade, input.color.a);
}

@fragment
fn fs_point(input: VertexOutput) -> @location(0) vec4<f32> {
    if camera.clip_enabled.x > 0.5 &&
       (any(input.relative < camera.clip_min.xyz) || any(input.relative > camera.clip_max.xyz)) {
        discard;
    }
    let radius_squared = dot(input.local, input.local);
    if radius_squared > 1.0 {
        discard;
    }
    let normal = vec3<f32>(
        input.local.x,
        -input.local.y,
        sqrt(max(0.0, 1.0 - radius_squared))
    );
    let light = normalize(vec3<f32>(-0.45, 0.65, 0.85));
    let shade = 0.72 + 0.28 * max(dot(normal, light), 0.0);
    let glint = pow(max(dot(normal, normalize(vec3<f32>(-0.25, 0.45, 1.0))), 0.0), 24.0) * 0.12;
    return vec4<f32>(input.color.rgb * shade + vec3<f32>(glint), input.color.a);
}

@vertex
fn vs_composite(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    var corner = vec2<f32>(-1.0, -1.0);
    if vertex == 1u {
        corner = vec2<f32>(3.0, -1.0);
    } else if vertex == 2u {
        corner = vec2<f32>(-1.0, 3.0);
    }
    return vec4<f32>(corner, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(position.xy);
    let dimensions = vec2<i32>(textureDimensions(scene_color, 0));
    if xy.x < 0 || xy.y < 0 || xy.x >= dimensions.x || xy.y >= dimensions.y {
        discard;
    }
    let color = textureLoad(scene_color, xy, 0);
    if color.a <= 0.001 {
        discard;
    }
    if camera.clip_enabled.y < 0.5 {
        return color;
    }
    let depth = textureLoad(scene_depth, xy, 0);
    var edge = 0.0;
    for (var row = -1; row <= 1; row += 1) {
        for (var column = -1; column <= 1; column += 1) {
            if row == 0 && column == 0 {
                continue;
            }
            let neighbor = xy + vec2<i32>(column, row);
            if neighbor.x < 0 || neighbor.y < 0 ||
               neighbor.x >= dimensions.x || neighbor.y >= dimensions.y {
                continue;
            }
            let neighbor_depth = textureLoad(scene_depth, neighbor, 0);
            let contrast = min(max(neighbor_depth - depth, 0.0) * 48.0, 1.0);
            let weight = select(1.0, 0.10, neighbor_depth >= 0.999);
            edge += contrast * weight;
        }
    }
    let shade = max(exp(-edge * 0.42 * camera.clip_enabled.z), 0.50);
    return vec4<f32>(color.rgb * shade, color.a);
}
