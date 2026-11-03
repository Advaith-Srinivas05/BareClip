// Draws one picture on the canvas, turning its stored values into colours.

struct Layer {
    // Left, top, right and bottom, from -1 to 1.
    place: vec4<f32>,
    to_red: vec4<f32>,
    to_green: vec4<f32>,
    to_blue: vec4<f32>,
    opacity: f32,
};

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var y_plane: texture_2d<f32>;
@group(0) @binding(2) var u_plane: texture_2d<f32>;
@group(0) @binding(3) var v_plane: texture_2d<f32>;
@group(0) @binding(4) var smooth_sampler: sampler;

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) spot: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> Corner {
    let spot = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var corner: Corner;
    corner.position = vec4<f32>(
        mix(layer.place.x, layer.place.z, spot.x),
        mix(layer.place.y, layer.place.w, spot.y),
        0.0,
        1.0,
    );
    corner.spot = spot;
    return corner;
}

@fragment
fn fragment_main(corner: Corner) -> @location(0) vec4<f32> {
    let stored = vec4<f32>(
        textureSample(y_plane, smooth_sampler, corner.spot).r,
        textureSample(u_plane, smooth_sampler, corner.spot).r,
        textureSample(v_plane, smooth_sampler, corner.spot).r,
        1.0,
    );
    return vec4<f32>(
        dot(layer.to_red, stored),
        dot(layer.to_green, stored),
        dot(layer.to_blue, stored),
        layer.opacity,
    );
}
