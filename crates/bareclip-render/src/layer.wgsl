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
    // Its filter: each row makes one colour from red, green and blue plus the last number, and then
    // each colour goes through a curve of nine evenly spaced points, packed four to a row.
    mix_red: vec4<f32>,
    mix_green: vec4<f32>,
    mix_blue: vec4<f32>,
    curves: array<vec4<f32>, 7>,
    // The clip's own exposure, contrast, saturation and temperature.
    adjust: vec4<f32>,
    // How much of the filter to use, how many seconds into the clip this is, how long the clip
    // lasts, and 1 if any effect is on.
    amounts: vec4<f32>,
    // The size of the picture as stored, and of the place it is drawn, in pixels.
    sizes: vec4<f32>,
    // Which effects are on, one bit each in the panel's order.
    effects_on: vec4<u32>,
    // Each effect's settings, as shares of their ranges.
    settings: array<vec4<f32>, 27>,
};

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var y_plane: texture_2d<f32>;
@group(0) @binding(2) var u_plane: texture_2d<f32>;
@group(0) @binding(3) var v_plane: texture_2d<f32>;
@group(0) @binding(4) var smooth_sampler: sampler;
// A small blurred copy of what the place shows, for the effects that blur. Colours are scaled by
// how much of each shows.
@group(0) @binding(5) var blurred: texture_2d<f32>;

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

// Covers the whole target, for drawing the small copy that gets blurred.
@vertex
fn vertex_full(@builtin(vertex_index) index: u32) -> Corner {
    let spot = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var corner: Corner;
    corner.position = vec4<f32>(spot.x * 2.0 - 1.0, 1.0 - spot.y * 2.0, 0.0, 1.0);
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

fn curve_point(index: u32) -> f32 {
    return layer.curves[index / 4u][index % 4u];
}

// One colour through its curve, the `channel`th of three.
fn curve(value: f32, channel: u32) -> f32 {
    let spot = clamp(value, 0.0, 1.0) * 8.0;
    let index = min(u32(spot), 7u);
    let along = spot - f32(index);
    let first = channel * 9u + index;
    return mix(curve_point(first), curve_point(first + 1u), along);
}

fn filtered(colour: vec3<f32>) -> vec3<f32> {
    let mixed = vec3<f32>(
        dot(layer.mix_red.xyz, colour) + layer.mix_red.w,
        dot(layer.mix_green.xyz, colour) + layer.mix_green.w,
        dot(layer.mix_blue.xyz, colour) + layer.mix_blue.w,
    );
    return vec3<f32>(curve(mixed.r, 0u), curve(mixed.g, 1u), curve(mixed.b, 2u));
}

// What the clip's filter and adjustments do. The preview and the export both come through here.
fn graded(colour: vec3<f32>) -> vec3<f32> {
    let strength = layer.amounts.x;
    var rgb = clamp(mix(colour, filtered(colour), strength), vec3<f32>(0.0), vec3<f32>(1.0));
    rgb = basic_grade(rgb, layer.adjust);
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

// The effects. Each is numbered as in the panel, and its settings are shares of their ranges. The
// numbers that shape them were measured from a sample recording of each effect at its starting
// settings, so the starting settings look like the recording and the rest scale from there.

const FLASH: u32 = 0u;
const VHS: u32 = 3u;
const VAPORWAVE: u32 = 5u;
const CHROMATIC: u32 = 6u;
const GREEN_SCREEN: u32 = 10u;
const BLACK_WHITE: u32 = 11u;
const FILMIC: u32 = 14u;
const GLITCH: u32 = 15u;
const DISCO: u32 = 16u;
const COLOUR_SHIFT: u32 = 17u;
const COMIC: u32 = 19u;
const RETRO: u32 = 20u;
const VERTICAL: u32 = 21u;
const RADIAL: u32 = 22u;
const SMOKE: u32 = 23u;
const KALEIDOSCOPE: u32 = 24u;
const BLUR: u32 = 12u;
const GLASS: u32 = 18u;
const GLOW: u32 = 25u;
const DIFFUSION: u32 = 26u;

const TURN: f32 = 6.2831853;

fn is_on(effect: u32) -> bool {
    return ((layer.effects_on.x >> effect) & 1u) == 1u;
}

fn setting(effect: u32, control: u32) -> f32 {
    return layer.settings[effect][control];
}

fn seconds() -> f32 {
    return layer.amounts.y;
}

// The place's width over its height.
fn aspect() -> f32 {
    return layer.sizes.z / max(layer.sizes.w, 1.0);
}

// A well-mixed whole number from another, after the PCG generator's output step.
fn mixed(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// A number from 0 to 1 that looks random but is always the same for the same spot. Spots an
// eighth apart or more get unrelated numbers.
fn hash(value: vec2<f32>) -> f32 {
    let across = u32(i32(floor(value.x * 8.0)));
    let down = u32(i32(floor(value.y * 8.0)));
    return f32(mixed(across ^ mixed(down)) >> 8u) / 16777216.0;
}

fn hash1(value: f32) -> f32 {
    return hash(vec2<f32>(value, value * 1.37 + 11.0));
}

fn smooth_noise(at: vec2<f32>) -> f32 {
    let cell = floor(at);
    let along = fract(at);
    let eased = along * along * (3.0 - 2.0 * along);
    let a = hash(cell);
    let b = hash(cell + vec2<f32>(1.0, 0.0));
    let c = hash(cell + vec2<f32>(0.0, 1.0));
    let d = hash(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, eased.x), mix(c, d, eased.x), eased.y);
}

fn clouds(at: vec2<f32>) -> f32 {
    var total = 0.0;
    var size = 1.0;
    var weight = 0.5;
    for (var octave = 0; octave < 4; octave++) {
        total += smooth_noise(at * size) * weight;
        size *= 2.0;
        weight *= 0.5;
    }
    return total;
}

// A spot on the place in steps of a 1080-high picture, so grain is the same size in the preview
// and the export.
fn grain_cell(spot: vec2<f32>) -> vec2<f32> {
    return floor(spot * vec2<f32>(aspect() * 1080.0, 1080.0));
}

fn grain(spot: vec2<f32>, salt: f32) -> f32 {
    let frame = floor(seconds() * 30.0);
    return hash(grain_cell(spot) + vec2<f32>(frame * 17.0 + salt, frame * 3.0)) - 0.5;
}

// The picture's colour and how much of it shows, at a spot on its place from 0 to 1 each way.
fn picture_at(spot: vec2<f32>) -> vec4<f32> {
    let inside = clamp(spot, vec2<f32>(0.0), vec2<f32>(1.0));
    let read = mix(layer.read_from, layer.read_to, inside);
    let first = textureSampleLevel(y_plane, smooth_sampler, read, 0.0);
    if layer.still > 0.5 {
        return vec4<f32>(first.rgb / max(first.a, 0.0001), first.a);
    }
    let stored = vec4<f32>(
        first.r,
        textureSampleLevel(u_plane, smooth_sampler, read, 0.0).r,
        textureSampleLevel(v_plane, smooth_sampler, read, 0.0).r,
        1.0,
    );
    return vec4<f32>(
        dot(layer.to_red, stored),
        dot(layer.to_green, stored),
        dot(layer.to_blue, stored),
        1.0,
    );
}

// Folds a spot back onto the picture, like a mirror at each edge, so nothing runs off it.
fn folded(spot: vec2<f32>) -> vec2<f32> {
    return 1.0 - abs(1.0 - fract(spot * 0.5) * 2.0);
}

// Whether Glitch is part way through a burst. Each quarter of a second, at the starting speed,
// either glitches or doesn't.
fn glitch_bursting() -> f32 {
    let clock = floor(seconds() * (2.0 + setting(GLITCH, 1u) * 10.0));
    return step(hash1(clock * 1.7 + 3.0), 0.15 + setting(GLITCH, 0u) * 0.9);
}

// Where on the picture each spot of the place reads from, after the effects that move it around.
// The second part is 0 where nothing shows.
struct Warp {
    spot: vec2<f32>,
    shows: f32,
};

fn warped(start: vec2<f32>) -> Warp {
    var spot = start;
    var shows = 1.0;
    let t = seconds();
    let wide = aspect();
    if is_on(KALEIDOSCOPE) {
        // The picture wrapped around the middle in mirrored wedges, its bottom at the middle and
        // its top outside, turning and drifting in and out.
        let count = round(2.0 + setting(KALEIDOSCOPE, 1u) * 32.0);
        let speed = 0.25 + setting(KALEIDOSCOPE, 0u) * 2.5;
        let from_middle = (spot - 0.5) * vec2<f32>(wide, 1.0);
        let wedge = TURN / count;
        var angle = atan2(from_middle.y, from_middle.x) + t * speed * 0.4;
        angle = angle - floor(angle / wedge) * wedge;
        let across = 1.0 - abs(angle / wedge * 2.0 - 1.0);
        let reach = length(from_middle) / (0.55 + 0.25 * sin(t * speed * 1.6));
        spot = vec2<f32>(across, 1.0 - folded(vec2<f32>(reach, 0.0)).x);
    }
    if is_on(VERTICAL) {
        // Five columns of small copies, each column sliding the other way to the one beside it.
        let count = 5.0;
        let column = floor(spot.x * count);
        let way = select(-1.0, 1.0, (u32(column) % 2u) == 0u);
        spot = fract(vec2<f32>(spot.x * count, spot.y * count + way * t * 0.5));
    }
    if is_on(RADIAL) {
        // Rows of small copies, each row further along than the last and drifting, with a gap
        // under each copy.
        let count = 5.0;
        let tilt = 0.08 * sin(t * 0.6);
        let turned = vec2<f32>(
            spot.x + (spot.y - 0.5) * tilt,
            spot.y - (spot.x - 0.5) * tilt,
        );
        let rows = turned.y * count * 0.8;
        let row = floor(rows);
        let across = turned.x * count + row * 0.35 + t * 0.4 * select(-1.0, 1.0, (u32(abs(row)) % 2u) == 0u);
        let down = fract(rows) / 0.8;
        shows *= step(down, 1.0);
        spot = vec2<f32>(fract(across), clamp(down, 0.0, 1.0));
    }
    if is_on(RETRO) {
        // Square blocks, about 80 across at the starting setting.
        let across = 320.0 * pow(0.0369, setting(RETRO, 0u));
        let blocks = vec2<f32>(across, max(1.0, round(across / wide)));
        spot = (floor(spot * blocks) + 0.5) / blocks;
    }
    if is_on(GLITCH) {
        // Now and then, for a fifth to half a second, bands of the picture jump sideways, as in
        // the recording, where it glitches about a third of the time.
        let strength = setting(GLITCH, 0u);
        let bursting = glitch_bursting();
        let tick = floor(t * (6.0 + setting(GLITCH, 1u) * 45.0));
        let band = floor(spot.y * 24.0);
        let moved = step(hash(vec2<f32>(band, tick)), 0.1 + strength * 0.4);
        spot.x += bursting * moved * (hash(vec2<f32>(tick, band)) - 0.5) * (0.05 + strength * 0.25);
    }
    if is_on(VHS) || is_on(VAPORWAVE) {
        // Tape rolls slightly from side to side, a little at a time.
        spot.x += 0.0015 * sin(t * 2.1 + spot.y * 6.0);
    }
    return Warp(spot, shows);
}

// The picture smeared sideways, as on a worn tape, over about `width` of the place.
fn smeared(spot: vec2<f32>, width: f32) -> vec4<f32> {
    var total = vec4<f32>(0.0);
    for (var tap = -3; tap <= 3; tap++) {
        total += picture_at(spot + vec2<f32>(f32(tap) * width / 6.0, 0.0));
    }
    return total / 7.0;
}

fn tape_colour(rgb: vec3<f32>, saturation: f32, gain: vec3<f32>, lift: vec3<f32>) -> vec3<f32> {
    let grey = vec3<f32>(dot(rgb, LUMA));
    return (grey + (rgb - grey) * saturation) * gain + lift;
}

fn hue_turned(rgb: vec3<f32>, angle: f32) -> vec3<f32> {
    let axis = vec3<f32>(0.57735);
    let c = cos(angle);
    return rgb * c + cross(axis, rgb) * sin(angle) + axis * dot(axis, rgb) * (1.0 - c);
}

fn nearest_of(rgb: vec3<f32>, palette: u32) -> vec3<f32> {
    if palette == 1u {
        return vec3<f32>(step(0.5, dot(rgb, LUMA)));
    }
    if palette == 2u {
        var colours = array<vec3<f32>, 16>(
            vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 0.667), vec3<f32>(0.0, 0.667, 0.0),
            vec3<f32>(0.0, 0.667, 0.667), vec3<f32>(0.667, 0.0, 0.0), vec3<f32>(0.667, 0.0, 0.667),
            vec3<f32>(0.667, 0.333, 0.0), vec3<f32>(0.667, 0.667, 0.667), vec3<f32>(0.333, 0.333, 0.333),
            vec3<f32>(0.333, 0.333, 1.0), vec3<f32>(0.333, 1.0, 0.333), vec3<f32>(0.333, 1.0, 1.0),
            vec3<f32>(1.0, 0.333, 0.333), vec3<f32>(1.0, 0.333, 1.0), vec3<f32>(1.0, 1.0, 0.333),
            vec3<f32>(1.0, 1.0, 1.0),
        );
        var best = colours[0];
        var nearest = 9.0;
        for (var index = 0; index < 16; index++) {
            let apart = distance(rgb, colours[index]);
            if apart < nearest {
                nearest = apart;
                best = colours[index];
            }
        }
        return best;
    }
    if palette == 3u {
        let levels = vec3<f32>(7.0, 7.0, 3.0);
        return round(rgb * levels) / levels;
    }
    if palette == 4u {
        let levels = vec3<f32>(31.0, 63.0, 31.0);
        return round(rgb * levels) / levels;
    }
    if palette == 5u {
        // A paint program's box of colours from the mid nineties.
        var colours = array<vec3<f32>, 20>(
            vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(0.502, 0.502, 0.502), vec3<f32>(0.502, 0.0, 0.0),
            vec3<f32>(0.502, 0.502, 0.0), vec3<f32>(0.0, 0.502, 0.0), vec3<f32>(0.0, 0.502, 0.502),
            vec3<f32>(0.0, 0.0, 0.502), vec3<f32>(0.502, 0.0, 0.502), vec3<f32>(0.502, 0.502, 0.251),
            vec3<f32>(0.0, 0.251, 0.251), vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(0.753, 0.753, 0.753),
            vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0),
            vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 1.0),
            vec3<f32>(1.0, 1.0, 0.502), vec3<f32>(1.0, 0.502, 0.251),
        );
        var best = colours[0];
        var nearest = 9.0;
        for (var index = 0; index < 20; index++) {
            let apart = distance(rgb, colours[index]);
            if apart < nearest {
                nearest = apart;
                best = colours[index];
            }
        }
        return best;
    }
    if palette == 6u {
        // Four shades of green, as on an old handheld game's screen.
        let shade = clamp(floor(dot(rgb, LUMA) * 4.0), 0.0, 3.0);
        var greens = array<vec3<f32>, 4>(
            vec3<f32>(0.059, 0.220, 0.059), vec3<f32>(0.188, 0.384, 0.188),
            vec3<f32>(0.545, 0.675, 0.059), vec3<f32>(0.608, 0.737, 0.059),
        );
        return greens[u32(shade)];
    }
    return rgb;
}

// How far through its rise Smoke is: black at the clip's ends, half bright in the middle.
fn smoke_amount() -> f32 {
    let t = seconds();
    let length = max(layer.amounts.z, 0.01);
    let squeeze = min(1.0, length / 5.0);
    let rise = smoothstep(0.83 * squeeze, 2.0 * squeeze, t);
    let fall = 1.0 - smoothstep(length - 2.0 * squeeze, length - 0.83 * squeeze, t);
    return rise * fall;
}

// Disco's tints and how long each lasts, as measured, over and over.
fn disco_tint() -> vec3<f32> {
    var lasts = array<f32, 10>(0.2, 0.5, 0.3, 0.4, 0.5, 0.3, 0.5, 0.3, 0.4, 0.5);
    var tints = array<vec3<f32>, 10>(
        vec3<f32>(1.0, 1.0, 0.4), vec3<f32>(0.7, 0.4, 0.95), vec3<f32>(0.42, 1.0, 0.55),
        vec3<f32>(0.42, 1.0, 0.9), vec3<f32>(0.42, 0.4, 0.93), vec3<f32>(0.95, 0.4, 0.55),
        vec3<f32>(0.95, 0.55, 0.38), vec3<f32>(0.95, 0.95, 0.37), vec3<f32>(0.7, 0.4, 0.95),
        vec3<f32>(0.42, 1.0, 0.55),
    );
    var left = seconds() % 3.9;
    for (var index = 0; index < 10; index++) {
        if left < lasts[index] {
            return tints[index];
        }
        left -= lasts[index];
    }
    return tints[0];
}

// The blurred copy at a spot, with its colours as they are.
fn blurred_at(spot: vec2<f32>) -> vec4<f32> {
    let stored = textureSampleLevel(blurred, smooth_sampler, clamp(spot, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    return vec4<f32>(stored.rgb / max(stored.a, 0.0001), stored.a);
}

// Where Glass's pane is across the picture, as measured: it comes in at half a second, crosses
// fast, slows near the right and leaves by 2.8 seconds. It comes round again every 3.
fn glass_middle(seconds: f32) -> f32 {
    var times = array<f32, 7>(0.5, 1.0, 1.25, 1.5, 2.0, 2.5, 2.8);
    var places = array<f32, 7>(-0.2, 0.28, 0.5, 0.75, 0.8, 0.88, 1.3);
    if seconds <= times[0] {
        return places[0];
    }
    for (var index = 1; index < 7; index++) {
        if seconds <= times[index] {
            let along = (seconds - times[index - 1]) / (times[index] - times[index - 1]);
            return mix(places[index - 1], places[index], along);
        }
    }
    return places[6];
}

// Glass: over the blurred picture, a sharp slanted pane, magnified, with a shadow on its
// trailing edge.
fn glass_at(spot: vec2<f32>, soft: vec4<f32>) -> vec4<f32> {
    let middle = glass_middle(seconds() % 3.0);
    let along = spot.x + (spot.y - 0.5) * 0.15 - middle;
    let inside = 1.0 - smoothstep(0.16, 0.165, abs(along));
    let through = vec2<f32>(middle, 0.5) + (spot - vec2<f32>(middle, 0.5)) / 1.3;
    var sharp = picture_at(through);
    let shadow = smoothstep(0.1, 0.165, along) * inside;
    sharp = vec4<f32>(sharp.rgb * (1.0 - 0.45 * shadow), sharp.a);
    let behind = smoothstep(0.165, 0.2, along) * (1.0 - smoothstep(0.2, 0.26, along));
    var out = mix(soft, sharp, inside);
    out = vec4<f32>(out.rgb * (1.0 - 0.3 * behind), out.a);
    return out;
}

fn with_effects(start: vec2<f32>, edge_fade: f32) -> vec4<f32> {
    let t = seconds();
    let wide = aspect();
    let warp = warped(start);
    let spot = warp.spot;
    var picture = picture_at(spot);
    let soft = blurred_at(spot);
    if is_on(BLUR) {
        picture = soft;
    }
    if is_on(GLASS) {
        picture = glass_at(spot, soft);
    }
    let tape = is_on(VHS) || is_on(VAPORWAVE);
    if tape {
        let strength = select(setting(VAPORWAVE, 2u), setting(VHS, 1u), is_on(VHS)) * 2.0;
        picture = mix(picture, smeared(spot, 0.012), clamp(strength, 0.0, 1.0));
    }
    if is_on(CHROMATIC) {
        // Red and blue part to either side, swinging over about three seconds.
        let apart = setting(CHROMATIC, 0u) * 0.073 * (0.7 + 0.3 * cos(TURN * (t - 1.25) / 3.0));
        picture.r = picture_at(spot + vec2<f32>(apart, 0.0)).r;
        picture.b = picture_at(spot - vec2<f32>(apart, 0.0)).b;
    }
    if tape {
        let split = 0.003 * sin(t * TURN / 3.0);
        picture.r = mix(picture.r, picture_at(spot + vec2<f32>(split, 0.0)).r, 0.5);
    }
    var rgb = picture.rgb;
    var shows = picture.a * warp.shows;
    if layer.graded > 0.5 {
        rgb = graded(rgb);
    }
    if is_on(GLOW) {
        // Bright parts bloom, as measured: a tight blur of what is brighter than about 0.4,
        // screened over the picture.
        let amount = setting(GLOW, 0u) / 0.61;
        let bloom = clamp((soft.rgb - 0.391) / 0.609, vec3<f32>(0.0), vec3<f32>(1.0)) * 2.1 * amount;
        let base = rgb * 0.98;
        rgb = 1.0 - (1.0 - base) * (1.0 - clamp(bloom, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    if is_on(DIFFUSION) {
        // A wide haze from the brightest parts, as measured, screened over the picture.
        let amount = setting(DIFFUSION, 0u) / 0.72;
        let bright = clamp((soft.rgb - 0.319) / 0.681, vec3<f32>(0.0), vec3<f32>(1.0));
        let haze = pow(bright, vec3<f32>(2.79)) * 2.9 * amount;
        rgb = 1.0 - (1.0 - rgb) * (1.0 - clamp(haze, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    if is_on(COMIC) {
        // Flat colours with dark shadows, inked edges and a screen of dots.
        let nudge = vec2<f32>(1.5 / (1080.0 * wide), 1.5 / 1080.0);
        let around = dot(picture_at(spot + vec2<f32>(nudge.x, 0.0)).rgb, LUMA)
            - dot(picture_at(spot - vec2<f32>(nudge.x, 0.0)).rgb, LUMA);
        let up_down = dot(picture_at(spot + vec2<f32>(0.0, nudge.y)).rgb, LUMA)
            - dot(picture_at(spot - vec2<f32>(0.0, nudge.y)).rgb, LUMA);
        // Dark parts go black, as measured.
        rgb = tape_colour(rgb, 0.851, vec3<f32>(1.11, 1.379, 1.345), vec3<f32>(-0.212, -0.314, -0.281));
        rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        let ink = smoothstep(0.06, 0.18, length(vec2<f32>(around, up_down)));
        rgb *= 1.0 - ink * 0.85;
        let cells = spot * vec2<f32>(wide, 1.0) * 90.0;
        let turned = vec2<f32>(cells.x + cells.y, cells.y - cells.x) * 0.7071;
        let dot_spot = fract(turned) - 0.5;
        let darkness = 1.0 - dot(rgb, LUMA);
        let dot_mark = 1.0 - smoothstep(0.2 + darkness * 0.25, 0.28 + darkness * 0.25, length(dot_spot));
        rgb *= 1.0 - dot_mark * setting(COMIC, 0u) * 0.8;
    }
    if is_on(VHS) {
        let strength = setting(VHS, 1u) * 2.0;
        let tinted = tape_colour(rgb, 0.748, vec3<f32>(0.967, 0.969, 0.977), vec3<f32>(-0.012));
        rgb = mix(rgb, tinted, clamp(strength, 0.0, 2.0));
        rgb += grain(spot, 1.0) * setting(VHS, 0u) * 0.06;
    }
    if is_on(VAPORWAVE) {
        let tint = setting(VAPORWAVE, 0u) / 0.29;
        let tinted = tape_colour(rgb, 0.893, vec3<f32>(0.893, 0.854, 0.932), vec3<f32>(0.034, -0.026, 0.188));
        rgb = mix(rgb, tinted, clamp(tint, 0.0, 3.0));
        rgb += grain(spot, 2.0) * setting(VAPORWAVE, 1u) * 0.08;
    }
    if tape {
        // Specks of tape noise along a band near the bottom.
        let band = smoothstep(0.9, 0.97, spot.y) * (1.0 - smoothstep(0.97, 1.0, spot.y));
        let speck = step(0.985, hash(grain_cell(spot) + vec2<f32>(floor(t * 30.0) * 7.0, 0.0)));
        rgb = mix(rgb, vec3<f32>(1.0), band * speck);
    }
    if is_on(FILMIC) {
        let strength = setting(FILMIC, 1u) * 2.0;
        let film = tape_colour(rgb, 0.61, vec3<f32>(1.177, 1.101, 1.01), vec3<f32>(0.03, 0.045, 0.046));
        rgb = mix(rgb, film, clamp(strength, 0.0, 2.0));
        rgb += grain(spot, 3.0) * setting(FILMIC, 0u) * 0.05;
    }
    if is_on(RETRO) {
        rgb = nearest_of(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), u32(round(setting(RETRO, 1u))));
    }
    if is_on(GLITCH) {
        // Bars of bright colour now and then, in the bands that jump.
        let strength = setting(GLITCH, 0u);
        let bursting = glitch_bursting();
        let tick = floor(t * (6.0 + setting(GLITCH, 1u) * 45.0));
        let band = floor(spot.y * 60.0);
        let bar = bursting * step(hash(vec2<f32>(band * 3.1, tick)), 0.02 + strength * 0.08);
        let colour = select(vec3<f32>(0.6, 0.3, 1.0), vec3<f32>(0.55, 1.0, 0.3), hash(vec2<f32>(band, tick * 2.0)) > 0.5);
        rgb = mix(rgb, colour, bar * 0.9);
    }
    if is_on(DISCO) {
        rgb *= disco_tint();
    }
    if is_on(COLOUR_SHIFT) {
        rgb = hue_turned(rgb, TURN * t / 2.0);
    }
    if is_on(KALEIDOSCOPE) && setting(KALEIDOSCOPE, 2u) > 0.5 {
        rgb = hue_turned(rgb, TURN * t / 4.0);
    }
    if is_on(SMOKE) {
        // Drifting smoke over a darkened picture, in from black and back out at the end.
        let drift = vec2<f32>(start.x * wide * 2.5 + t * 0.15, start.y * 2.5 - t * 0.05);
        let smoke = clouds(drift);
        // Coming in and going out, the picture shows first through gaps in the smoke.
        let shown = clamp(smoke_amount() * (0.6 + 0.8 * smoke), 0.0, 1.0);
        rgb *= (0.25 + 0.6 * smoke) * shown;
    }
    if is_on(FLASH) {
        // Black for half of each flash, with a hard cut unless Fade is ticked.
        let rate = pow(2.0, 3.0 * (setting(FLASH, 0u) - 0.23));
        let phase = fract(t * rate + 0.733);
        var dark = step(phase, 0.5);
        if setting(FLASH, 1u) > 0.5 {
            dark = 0.5 + 0.5 * cos(TURN * (phase - 0.25));
        }
        rgb *= 1.0 - dark;
    }
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if is_on(GREEN_SCREEN) {
        let threshold = setting(GREEN_SCREEN, 0u);
        let which = u32(round(setting(GREEN_SCREEN, 1u)));
        var others = max(rgb.g, rgb.b);
        var key = rgb.r;
        if which == 1u {
            others = max(rgb.r, rgb.b);
            key = rgb.g;
        } else if which == 2u {
            others = max(rgb.r, rgb.g);
            key = rgb.b;
        }
        let lead = key - others;
        let limit = 0.3 - threshold * 0.28;
        shows *= 1.0 - smoothstep(limit, limit + 0.08, lead);
    }
    if is_on(BLACK_WHITE) {
        let threshold = setting(BLACK_WHITE, 0u) * 0.5;
        let light = dot(rgb, LUMA);
        if setting(BLACK_WHITE, 1u) > 0.5 {
            shows *= 1.0 - smoothstep(1.0 - threshold - 0.05, 1.0 - threshold + 0.05, light);
        } else {
            shows *= smoothstep(threshold - 0.05, threshold + 0.05, light);
        }
    }
    return vec4<f32>(rgb, shows * edge_fade);
}

// The picture with its colours graded and nothing else, scaled by how much of it shows, as the
// copy that gets blurred.
@fragment
fn fragment_source(corner: Corner) -> @location(0) vec4<f32> {
    let picture = picture_at(corner.spot);
    var rgb = picture.rgb;
    if layer.graded > 0.5 {
        rgb = graded(rgb);
    }
    return vec4<f32>(rgb * picture.a, picture.a);
}

@fragment
fn fragment_main(corner: Corner) -> @location(0) vec4<f32> {
    // How many pixels this is in from the nearest edge, for softening the jagged edge a turned
    // picture would otherwise have.
    let inward = min(corner.spot, 1.0 - corner.spot) / max(fwidth(corner.spot), vec2<f32>(0.0001));
    let edge = clamp(min(inward.x, inward.y), 0.0, 1.0);
    let fade = layer.opacity * mix(1.0, edge, layer.soft_edges);
    if layer.amounts.w > 0.5 {
        return with_effects(corner.spot, fade);
    }
    let read = mix(layer.read_from, layer.read_to, corner.spot);
    let first = textureSample(y_plane, smooth_sampler, read);
    let stored = vec4<f32>(
        first.r,
        textureSample(u_plane, smooth_sampler, read).r,
        textureSample(v_plane, smooth_sampler, read).r,
        1.0,
    );
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
