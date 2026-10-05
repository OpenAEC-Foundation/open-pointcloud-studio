struct Camera {
    // The w components hold the eye position relative to the scene centre,
    // measured along each axis.
    right: vec4<f32>,
    up: vec4<f32>,
    toward: vec4<f32>,
    projection: vec4<f32>, // local width, height, scale, quarter of the depth range
    view: vec4<f32>,       // pan x, pan y, point size, display scale
    surface: vec4<f32>,     // widget x, y, target physical width, height
    clip_min: vec4<f32>,
    clip_max: vec4<f32>,
    clip_enabled: vec4<f32>, // section on (2 when turned), eye-dome on, eye-dome strength, sRGB target
    splat: vec4<f32>,        // walking: point radius in the scene, largest radius in pixels
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
    @location(4) point_depth: f32,
    @location(5) point_world_radius: f32,
};

// Whether a position lies outside the section box. A turned box holds the
// sine of its turn in clip_min.w and the cosine in clip_max.w; the position is
// turned back about the centre of the box before the limits are tested.
fn outside_section(relative: vec3<f32>) -> bool {
    return outside_section_by(relative, 0.0);
}

// The same with a margin outside the limits that still counts as inside.
fn outside_section_by(relative: vec3<f32>, slack: f32) -> bool {
    if camera.clip_enabled.x < 0.5 {
        return false;
    }
    var at = relative;
    if camera.clip_enabled.x > 1.5 {
        let center = (camera.clip_min.xyz + camera.clip_max.xyz) * 0.5;
        let offset = relative.xy - center.xy;
        let sine = camera.clip_min.w;
        let cosine = camera.clip_max.w;
        at = vec3<f32>(
            center.x + cosine * offset.x + sine * offset.y,
            center.y - sine * offset.x + cosine * offset.y,
            relative.z
        );
    }
    return any(at < camera.clip_min.xyz - vec3<f32>(slack)) ||
        any(at > camera.clip_max.xyz + vec3<f32>(slack));
}

// A mesh fragment's position is interpolated, and rounds a little to either
// side of a face of the box that its triangle lies in, such as the outside
// of a wall at the edge of the model when the box spans it. It is tested with
// a margin far below a millimetre in a building, so that such a face is
// kept whole.
fn mesh_slack() -> f32 {
    let reach = max(abs(camera.clip_min.xyz), abs(camera.clip_max.xyz));
    return 1e-5 * max(max(reach.x, reach.y), max(reach.z, 1.0));
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let depth = camera.toward.w - dot(input.relative.xyz, camera.toward.xyz);
    var output: VertexOutput;
    output.color = input.color;
    output.local = vec2<f32>(0.0, 0.0);
    output.relative = input.relative.xyz;
    output.normal = vec3<f32>(0.0, 0.0, 0.0);
    output.point_depth = depth;
    output.point_world_radius = 0.0;
    if depth <= 0.01 {
        output.position = vec4<f32>(2.0, 2.0, 1.0, 1.0);
        return output;
    }

    let local_x = camera.projection.x * 0.5 + camera.view.x
        + (dot(input.relative.xyz, camera.right.xyz) - camera.right.w) * camera.projection.z / depth;
    let local_y = camera.projection.y * 0.5 + camera.view.y
        - (dot(input.relative.xyz, camera.up.xyz) - camera.up.w) * camera.projection.z / depth;
    let physical = (camera.surface.xy + vec2<f32>(local_x, local_y)) * camera.view.w;
    let center = vec2<f32>(
        physical.x / camera.surface.z * 2.0 - 1.0,
        1.0 - physical.y / camera.surface.w * 2.0
    );
    // While walking a point keeps a size in the scene, so the surfaces close
    // by fill in; elsewhere it has the chosen size on screen.
    let pixel_radius = clamp(
        camera.splat.x * camera.projection.z / depth,
        camera.view.z,
        max(camera.view.z, camera.splat.y)
    );
    let radius = vec2<f32>(
        pixel_radius * camera.view.w / camera.surface.z,
        pixel_radius * camera.view.w / camera.surface.w
    );
    output.point_world_radius = pixel_radius * depth / camera.projection.z;
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
    let depth = camera.toward.w - dot(input.relative.xyz, camera.toward.xyz);
    var output: VertexOutput;
    output.color = input.color;
    output.local = vec2<f32>(0.0, 0.0);
    output.relative = input.relative.xyz;
    output.normal = input.normal.xyz;
    output.point_depth = depth;
    output.point_world_radius = 0.0;
    // The position on screen is the physical pixel
    //   offset + across * scale / depth
    // with the distance along the view as the w of the position, so that
    // what the fragments receive is interpolated in perspective: the depth
    // and the position stay exact inside a large triangle, as in a cap over
    // a cut wall or a wall of a few triangles. Every component is then
    // linear in the corner, so the device cuts a triangle off where it
    // passes the near plane instead of losing it whole when a corner lies
    // behind the eye.
    let scale = camera.projection.z * camera.view.w;
    let offset = (camera.surface.xy + camera.projection.xy * 0.5 + camera.view.xy) * camera.view.w;
    let across = vec2<f32>(
        dot(input.relative.xyz, camera.right.xyz) - camera.right.w,
        camera.up.w - dot(input.relative.xyz, camera.up.xyz)
    ) * scale;
    let physical_w = offset * depth + across;
    output.position = vec4<f32>(
        physical_w.x / camera.surface.z * 2.0 - depth,
        depth - physical_w.y / camera.surface.w * 2.0,
        depth - 0.01,
        depth
    );
    return output;
}

struct PointFragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(input: VertexOutput) -> PointFragment {
    if outside_section_by(input.relative, mesh_slack()) {
        discard;
    }
    var output: PointFragment;
    // Depth grows with the distance as for the points, not as one over it.
    output.depth = clamp(input.point_depth / (camera.projection.w * 4.0), 0.0, 1.0);
    if dot(input.normal, input.normal) <= 0.000001 {
        output.color = input.color;
        return output;
    }
    let light = normalize(vec3<f32>(-0.35, -0.25, 0.90));
    let shade = 0.76 + 0.24 * max(dot(normalize(input.normal), light), 0.0);
    output.color = vec4<f32>(input.color.rgb * shade, input.color.a);
    return output;
}

@fragment
fn fs_point(input: VertexOutput) -> PointFragment {
    if outside_section(input.relative) {
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
    let front_depth = max(0.01, input.point_depth - input.point_world_radius * normal.z);
    var output: PointFragment;
    output.color = vec4<f32>(input.color.rgb * shade + vec3<f32>(glint), input.color.a);
    output.depth = clamp(front_depth / (camera.projection.w * 4.0), 0.0, 1.0);
    return output;
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

// Scene colours are stored as their encoded bytes. A frame target that
// encodes to sRGB on write would brighten them a second time, so decode first.
fn display(color: vec3<f32>) -> vec3<f32> {
    if camera.clip_enabled.w > 0.5 {
        return pow(color, vec3<f32>(2.2));
    }
    return color;
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
        return vec4<f32>(display(color.rgb), color.a);
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
    return vec4<f32>(display(color.rgb * shade), color.a);
}
