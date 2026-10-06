// A photo laid over the points from where it was taken. Every pixel of the
// view looks along a direction; the photo's own camera maps that direction to
// one of its pixels, whichever way the view looks.

struct Camera {
    right: vec4<f32>,
    up: vec4<f32>,
    toward: vec4<f32>,
    projection: vec4<f32>, // local width, height, focal length, quarter of the depth range
    view: vec4<f32>,       // pan x, pan y, point size, display scale
    surface: vec4<f32>,    // widget x, y, target physical width, height
    clip_min: vec4<f32>,
    clip_max: vec4<f32>,
    clip_enabled: vec4<f32>, // w: the target encodes to sRGB
    splat: vec4<f32>,
};

struct Photo {
    axis_x: vec4<f32>, // the camera's X axis in the scene; w: 0 pinhole, 1 spherical, 2 cylindrical
    axis_y: vec4<f32>, // the camera's Y axis; w: how much of the photo covers the points
    axis_z: vec4<f32>, // the camera's Z axis; w: texels of the texture per radian
    size: vec4<f32>,   // width and height as stated; whether the columns go round; levels
    lens: vec4<f32>,   // pinhole: focal across, down, principal column, row
                       // spherical: angle of a pixel across, down
                       // cylindrical: angle across, height down, radius, principal row
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> photo: Photo;
@group(1) @binding(1) var photo_texture: texture_2d<f32>;
@group(1) @binding(2) var photo_sampler: sampler;

@vertex
fn vs_photo(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    var corner = vec2<f32>(-1.0, -1.0);
    if vertex == 1u {
        corner = vec2<f32>(3.0, -1.0);
    } else if vertex == 2u {
        corner = vec2<f32>(-1.0, 3.0);
    }
    return vec4<f32>(corner, 0.0, 1.0);
}

// Texture coordinates of a direction in the frame of the photo's camera;
// below zero where the photo does not see it.
fn place(local: vec3<f32>) -> vec2<f32> {
    let kind = photo.axis_x.w;
    let size = photo.size.xy;
    var column: f32 = 0.0;
    var row: f32 = 0.0;
    if kind < 0.5 {
        let depth = -local.z;
        if depth <= 0.0 {
            return vec2<f32>(-1.0, -1.0);
        }
        column = photo.lens.z + 0.5 + photo.lens.x * local.x / depth;
        row = photo.lens.w + 0.5 - photo.lens.y * local.y / depth;
    } else {
        let across = length(local.xy);
        column = size.x * 0.5 - atan2(local.y, local.x) / photo.lens.x;
        if photo.size.z > 0.5 {
            // Round the seam behind the camera: a rounding there must not
            // leave a column without the photo.
            column = clamp(column - size.x * floor(column / size.x), 0.0, size.x * 0.99999);
        }
        if kind < 1.5 {
            row = size.y * 0.5 - atan2(local.z, across) / photo.lens.y;
        } else {
            if across <= 0.0 {
                return vec2<f32>(-1.0, -1.0);
            }
            row = photo.lens.w + 0.5 - photo.lens.z * local.z / across / photo.lens.y;
        }
    }
    if column < 0.0 || row < 0.0 || column >= size.x || row >= size.y {
        return vec2<f32>(-1.0, -1.0);
    }
    return vec2<f32>(column / size.x, row / size.y);
}

@fragment
fn fs_photo(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let local_pixel = position.xy / camera.view.w - camera.surface.xy;
    let across = local_pixel.x - camera.projection.x * 0.5 - camera.view.x;
    let down = local_pixel.y - camera.projection.y * 0.5 - camera.view.y;
    let direction = camera.right.xyz * across
        - camera.up.xyz * down
        - camera.toward.xyz * camera.projection.z;
    let local = vec3<f32>(
        dot(direction, photo.axis_x.xyz),
        dot(direction, photo.axis_y.xyz),
        dot(direction, photo.axis_z.xyz)
    );
    let uv = place(local);
    if uv.x < 0.0 {
        discard;
    }
    // The level whose texels are about as large as a pixel of the screen.
    let level = clamp(
        log2(max(photo.axis_z.w / (camera.projection.z * camera.view.w), 1.0)),
        0.0,
        photo.size.w - 1.0
    );
    var color = textureSampleLevel(photo_texture, photo_sampler, uv, level).rgb;
    if camera.clip_enabled.w > 0.5 {
        // The target encodes to sRGB on write; photos are already encoded.
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, photo.axis_y.w);
}
