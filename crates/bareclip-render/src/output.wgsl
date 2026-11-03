// Turns the finished canvas into what encoders take: HD-standard values in the usual range,
// with brightness at full size and colour at a quarter of it.

@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var smooth_sampler: sampler;

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

@fragment
fn brightness_main(corner: Corner) -> @location(0) vec4<f32> {
    let colour = textureSample(canvas, smooth_sampler, corner.spot).rgb;
    let y = dot(colour, vec3<f32>(0.2126, 0.7152, 0.0722));
    return vec4<f32>(16.0 / 255.0 + y * 219.0 / 255.0, 0.0, 0.0, 1.0);
}

@fragment
fn colour_main(corner: Corner) -> @location(0) vec4<f32> {
    let colour = textureSample(canvas, smooth_sampler, corner.spot).rgb;
    let u = dot(colour, vec3<f32>(-0.2126, -0.7152, 0.9278)) / 1.8556;
    let v = dot(colour, vec3<f32>(0.7874, -0.7152, -0.0722)) / 1.5748;
    return vec4<f32>(
        128.0 / 255.0 + u * 224.0 / 255.0,
        128.0 / 255.0 + v * 224.0 / 255.0,
        0.0,
        1.0,
    );
}
