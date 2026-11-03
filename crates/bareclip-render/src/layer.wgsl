// Draws one picture on the canvas, turning its stored values into colours.

struct Layer {
    // The place's middle and half its size, in the canvas's pixels.
    middle: vec2<f32>,
    half_size: vec2<f32>,
    // The cosine and sine of its turn. Turning has to happen in pixels, because the canvas's
    // -1 to 1 isn't the same distance across and down.
    turn: vec2<f32>,
    canvas: vec2<f32>,
    // Where the picture is read from, from corner to corner. A flip swaps them.
    read_from: vec2<f32>,
    read_to: vec2<f32>,
    to_red: vec4<f32>,
    to_green: vec4<f32>,
    to_blue: vec4<f32>,
    opacity: f32,
    soft_edges: f32,
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
    let reach = (spot * 2.0 - 1.0) * layer.half_size;
    let pixel = layer.middle + vec2<f32>(
        reach.x * layer.turn.x - reach.y * layer.turn.y,
        reach.x * layer.turn.y + reach.y * layer.turn.x,
    );
    corner.position = vec4<f32>(
        pixel.x / layer.canvas.x * 2.0 - 1.0,
        1.0 - pixel.y / layer.canvas.y * 2.0,
        0.0,
        1.0,
    );
    corner.spot = spot;
    return corner;
}

@fragment
fn fragment_main(corner: Corner) -> @location(0) vec4<f32> {
    let read = mix(layer.read_from, layer.read_to, corner.spot);
    let stored = vec4<f32>(
        textureSample(y_plane, smooth_sampler, read).r,
        textureSample(u_plane, smooth_sampler, read).r,
        textureSample(v_plane, smooth_sampler, read).r,
        1.0,
    );
    // How many pixels this is in from the nearest edge, for softening the jagged edge a turned
    // picture would otherwise have.
    let inward = min(corner.spot, 1.0 - corner.spot) / max(fwidth(corner.spot), vec2<f32>(0.0001));
    let edge = clamp(min(inward.x, inward.y), 0.0, 1.0);
    return vec4<f32>(
        dot(layer.to_red, stored),
        dot(layer.to_green, stored),
        dot(layer.to_blue, stored),
        layer.opacity * mix(1.0, edge, layer.soft_edges),
    );
}
