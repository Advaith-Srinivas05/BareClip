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
    // 1 for an image, whose first plane holds colours and how see-through each pixel is.
    still: f32,
    // 1 if its colours have anything done to them, so plain clips skip all of it.
    graded: f32,
    // Exposure, contrast, saturation and temperature from its filter, each 0 for no change.
    filter_basic: vec4<f32>,
    shadows: vec4<f32>,
    highlights: vec4<f32>,
    // The colour dark parts become, with how far they go there in the last place.
    duotone_dark: vec4<f32>,
    duotone_light: vec4<f32>,
    // A colour washed over everything, with how much in the last place.
    wash: vec4<f32>,
    // The clip's own exposure, contrast, saturation and temperature.
    adjust: vec4<f32>,
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

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn basic_grade(colour: vec3<f32>, basic: vec4<f32>) -> vec3<f32> {
    var rgb = colour * pow(2.0, basic.x * 1.5);
    rgb = (rgb - vec3<f32>(0.5)) * (1.0 + basic.y) + vec3<f32>(0.5);
    rgb = mix(vec3<f32>(dot(rgb, LUMA)), rgb, 1.0 + basic.z);
    return rgb + vec3<f32>(basic.w * 0.15, 0.0, -basic.w * 0.15);
}

// What the clip's filter and adjustments do. The preview and the export both come through here.
fn graded(colour: vec3<f32>) -> vec3<f32> {
    var rgb = clamp(basic_grade(colour, layer.filter_basic), vec3<f32>(0.0), vec3<f32>(1.0));
    let luma = dot(rgb, LUMA);
    rgb *= mix(layer.shadows.rgb, layer.highlights.rgb, luma);
    rgb = mix(rgb, mix(layer.duotone_dark.rgb, layer.duotone_light.rgb, luma), layer.duotone_dark.a);
    rgb = mix(rgb, layer.wash.rgb * (0.3 + luma), layer.wash.a);
    rgb = basic_grade(rgb, layer.adjust);
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fragment_main(corner: Corner) -> @location(0) vec4<f32> {
    let read = mix(layer.read_from, layer.read_to, corner.spot);
    let first = textureSample(y_plane, smooth_sampler, read);
    let stored = vec4<f32>(
        first.r,
        textureSample(u_plane, smooth_sampler, read).r,
        textureSample(v_plane, smooth_sampler, read).r,
        1.0,
    );
    // How many pixels this is in from the nearest edge, for softening the jagged edge a turned
    // picture would otherwise have.
    let inward = min(corner.spot, 1.0 - corner.spot) / max(fwidth(corner.spot), vec2<f32>(0.0001));
    let edge = clamp(min(inward.x, inward.y), 0.0, 1.0);
    let fade = layer.opacity * mix(1.0, edge, layer.soft_edges);
    if layer.still > 0.5 {
        // Stored scaled by how see-through it is, which keeps smoothing from bleeding the colour
        // of invisible pixels into an edge. Blending wants it unscaled.
        let colours = first.rgb / max(first.a, 0.0001);
        if layer.graded > 0.5 {
            return vec4<f32>(graded(colours), first.a * fade);
        }
        return vec4<f32>(colours, first.a * fade);
    }
    let colours = vec3<f32>(
        dot(layer.to_red, stored),
        dot(layer.to_green, stored),
        dot(layer.to_blue, stored),
    );
    if layer.graded > 0.5 {
        return vec4<f32>(graded(colours), fade);
    }
    return vec4<f32>(colours, fade);
}
