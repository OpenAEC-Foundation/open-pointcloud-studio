// Station photos: textured balls at scanner stations and the view from
// inside one station. Every photo keeps its own pinhole pose; a direction is
// looked up in the photo that sees it furthest from its border.

struct Camera {
    // The w components hold the eye position relative to the scene centre,
    // measured along each axis.
    right: vec4<f32>,
    up: vec4<f32>,
    toward: vec4<f32>,
    projection: vec4<f32>, // local width, height, scale, quarter of the depth range
    view: vec4<f32>,       // pan x, pan y, point size, display scale
    surface: vec4<f32>,    // widget x, y, target physical width, height
    clip_min: vec4<f32>,   // w: first photo of the station being viewed
    clip_max: vec4<f32>,   // w: number of photos of that station
    clip_enabled: vec4<f32>,
    splat: vec4<f32>,
};

struct Face {
    right: vec4<f32>, // camera right * focal / width; w: principal column as a fraction
    up: vec4<f32>,    // camera up * focal / height; w: principal row as a fraction
    back: vec4<f32>,  // camera backward axis; w: texture layer
};

struct Faces {
    items: array<Face, 256>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> faces: Faces;
@group(1) @binding(1) var photo_texture: texture_2d_array<f32>;
@group(1) @binding(2) var photo_sampler: sampler;

fn photo(first: u32, count: u32, direction: vec3<f32>) -> vec3<f32> {
    var best = -1.0;
    var best_uv = vec2<f32>(0.5, 0.5);
    var best_layer = 0;
    for (var index = 0u; index < count; index += 1u) {
        let face = faces.items[first + index];
        let depth = -dot(direction, face.back.xyz);
        if depth <= 0.0 {
            continue;
        }
        let uv = vec2<f32>(
            dot(direction, face.right.xyz) / depth + face.right.w,
            -dot(direction, face.up.xyz) / depth + face.up.w
        );
        let margin = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
        if margin > best {
            best = margin;
            best_uv = uv;
            best_layer = i32(face.back.w + 0.5);
        }
    }
    if best < -0.01 {
        // No photo looks this way.
        return vec3<f32>(0.13, 0.13, 0.16);
    }
    return textureSampleLevel(
        photo_texture,
        photo_sampler,
        clamp(best_uv, vec2<f32>(0.0), vec2<f32>(1.0)),
        best_layer,
        0.0
    ).rgb;
}

struct BallInput {
    @builtin(vertex_index) vertex: u32,
    @location(0) placement: vec4<f32>, // centre relative to the scene centre, world radius
    @location(1) photos: vec4<f32>,    // first photo, photo count, smallest and largest pixel radius
};

struct BallOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) photos: vec2<u32>,
    @location(2) depth: f32,
    @location(3) world_radius: f32,
};

@vertex
fn vs_ball(input: BallInput) -> BallOutput {
    let relative = input.placement.xyz;
    let depth = camera.toward.w - dot(relative, camera.toward.xyz);
    var output: BallOutput;
    output.local = vec2<f32>(0.0, 0.0);
    output.photos = vec2<u32>(u32(input.photos.x + 0.5), u32(input.photos.y + 0.5));
    output.depth = depth;
    output.world_radius = 0.0;
    if depth <= 0.01 {
        output.position = vec4<f32>(2.0, 2.0, 1.0, 1.0);
        return output;
    }
    let local_x = camera.projection.x * 0.5 + camera.view.x
        + (dot(relative, camera.right.xyz) - camera.right.w) * camera.projection.z / depth;
    let local_y = camera.projection.y * 0.5 + camera.view.y
        - (dot(relative, camera.up.xyz) - camera.up.w) * camera.projection.z / depth;
    let physical = (camera.surface.xy + vec2<f32>(local_x, local_y)) * camera.view.w;
    let center = vec2<f32>(
        physical.x / camera.surface.z * 2.0 - 1.0,
        1.0 - physical.y / camera.surface.w * 2.0
    );
    let pixels = clamp(
        input.placement.w * camera.projection.z / depth,
        input.photos.z,
        input.photos.w
    );
    let radius = vec2<f32>(
        pixels * camera.view.w / camera.surface.z,
        pixels * camera.view.w / camera.surface.w
    );
    output.world_radius = pixels * depth / camera.projection.z;
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

struct BallFragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_ball(input: BallOutput) -> BallFragment {
    let radius_squared = dot(input.local, input.local);
    if radius_squared > 1.0 {
        discard;
    }
    let bulge = sqrt(max(0.0, 1.0 - radius_squared));
    // Look through the ball at its far inner surface: the photo then appears
    // the right way round, as it does from inside the station.
    let direction = camera.right.xyz * input.local.x
        + camera.up.xyz * input.local.y
        - camera.toward.xyz * bulge;
    var color = photo(input.photos.x, input.photos.y, direction) * (0.80 + 0.20 * bulge);
    let rim = smoothstep(0.66, 0.90, radius_squared);
    color = mix(color, vec3<f32>(0.96, 0.62, 0.04), rim);
    let front_depth = max(0.01, input.depth - input.world_radius * bulge);
    var output: BallFragment;
    output.color = vec4<f32>(color, 1.0);
    // Stations stay visible through walls and roofs, as their markers always
    // were: balls sort among themselves in a thin slice in front of the scene.
    output.depth = clamp(front_depth / (camera.projection.w * 4.0), 0.0, 1.0) * 0.001;
    return output;
}

@vertex
fn vs_sky(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    var corner = vec2<f32>(-1.0, -1.0);
    if vertex == 1u {
        corner = vec2<f32>(3.0, -1.0);
    } else if vertex == 2u {
        corner = vec2<f32>(-1.0, 3.0);
    }
    return vec4<f32>(corner, 0.0, 1.0);
}

@fragment
fn fs_sky(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let local = position.xy / camera.view.w - camera.surface.xy;
    let across = local.x - camera.projection.x * 0.5 - camera.view.x;
    let down = local.y - camera.projection.y * 0.5 - camera.view.y;
    let direction = camera.right.xyz * across
        - camera.up.xyz * down
        - camera.toward.xyz * camera.projection.z;
    var color = photo(u32(camera.clip_min.w + 0.5), u32(camera.clip_max.w + 0.5), direction);
    if camera.clip_enabled.w > 0.5 {
        // The target encodes to sRGB on write; photos are already encoded.
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, 1.0);
}
