// Blurs a small copy of a picture, one way at a time, and draws a blurred copy stretched over
// the whole canvas for Blur fill.

struct Blur {
    // One texel across or down, whichever way this pass blurs.
    step: vec2<f32>,
    // How wide the blur is, in texels.
    sigma: f32,
    // How much of the picture shows, for Blur fill.
    opacity: f32,
    // For Blur fill, where on the blurred picture each spot of the canvas reads from.
    fill_scale: vec2<f32>,
    fill_offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> blur: Blur;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var smooth_sampler: sampler;

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) spot: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> Corner {
    let spot = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var corner: Corner;
    corner.position = vec4<f32>(spot.x * 2.0 - 1.0, 1.0 - spot.y * 2.0, 0.0, 1.0);
    corner.spot = spot;
    return corner;
}

// A Gaussian blur along one way. Colours are stored scaled by how much of each shows, so
// see-through parts don't darken their neighbours.
@fragment
fn blur_main(corner: Corner) -> @location(0) vec4<f32> {
    let reach = i32(ceil(blur.sigma * 3.0));
    var total = vec4<f32>(0.0);
    var weights = 0.0;
    for (var tap = -12; tap <= 12; tap++) {
        if abs(tap) <= reach {
            let weight = exp(-f32(tap * tap) / (2.0 * blur.sigma * blur.sigma));
            total += textureSampleLevel(source, smooth_sampler, corner.spot + blur.step * f32(tap), 0.0) * weight;
            weights += weight;
        }
    }
    return total / max(weights, 0.0001);
}

// The blurred picture stretched to cover the canvas, keeping its shape, behind the picture itself.
@fragment
fn fill_main(corner: Corner) -> @location(0) vec4<f32> {
    let read = corner.spot * blur.fill_scale + blur.fill_offset;
    let stored = textureSampleLevel(source, smooth_sampler, clamp(read, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    return vec4<f32>(stored.rgb / max(stored.a, 0.0001), stored.a * blur.opacity);
}
