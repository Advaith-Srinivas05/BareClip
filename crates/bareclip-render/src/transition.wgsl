// Draws two clips as the two sides of a transition. Both have already been drawn on canvases of their
// own, with their colours scaled by how see-through they are, so what comes out is too.

struct Join {
    kind: u32,
    progress: f32,
    // The canvas's width over its height, so circles come out round.
    aspect: f32,
    unused: f32,
};

@group(0) @binding(0) var<uniform> join: Join;
@group(0) @binding(1) var going: texture_2d<f32>;
@group(0) @binding(2) var coming: texture_2d<f32>;
@group(0) @binding(3) var smooth_sampler: sampler;

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

const PI: f32 = 3.14159265;

fn inside(uv: vec2<f32>) -> bool {
    return uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0;
}

// The clip going out. Past its edges there is nothing.
fn a(uv: vec2<f32>) -> vec4<f32> {
    if !inside(uv) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(going, smooth_sampler, uv, 0.0);
}

// The clip coming in.
fn b(uv: vec2<f32>) -> vec4<f32> {
    if !inside(uv) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(coming, smooth_sampler, uv, 0.0);
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let along = fract(p);
    let eased = along * along * (3.0 - 2.0 * along);
    let top = mix(hash(cell), hash(cell + vec2<f32>(1.0, 0.0)), eased.x);
    let low = mix(hash(cell + vec2<f32>(0.0, 1.0)), hash(cell + vec2<f32>(1.0, 1.0)), eased.x);
    return mix(top, low, eased.y);
}

fn clouds(p: vec2<f32>) -> f32 {
    return noise(p) * 0.55 + noise(p * 2.1 + vec2<f32>(3.1, 1.7)) * 0.3
        + noise(p * 4.3 + vec2<f32>(7.3, 2.9)) * 0.15;
}

// How much of the coming clip shows where something has crossed an edge. `soft` is how wide the edge
// is, so a hard wipe still gets a smooth line.
fn crossed(distance: f32, soft: f32) -> f32 {
    return smoothstep(-soft, soft, distance);
}

fn between(uv: vec2<f32>, amount: f32) -> vec4<f32> {
    return mix(a(uv), b(uv), clamp(amount, 0.0, 1.0));
}

fn blurred(uv: vec2<f32>, reach: f32, from_coming: bool) -> vec4<f32> {
    var total = vec4<f32>(0.0);
    let steps = 12;
    for (var index = 0; index < steps; index += 1) {
        let turn = f32(index) / f32(steps) * 2.0 * PI;
        let far = (f32(index % 3) + 1.0) / 3.0;
        let offset = vec2<f32>(cos(turn) / join.aspect, sin(turn)) * reach * far;
        let at = clamp(uv + offset, vec2<f32>(0.0), vec2<f32>(1.0));
        if from_coming {
            total += b(at);
        } else {
            total += a(at);
        }
    }
    return total / f32(steps);
}

fn zoomed(uv: vec2<f32>, scale: f32) -> vec2<f32> {
    return (uv - vec2<f32>(0.5)) / scale + vec2<f32>(0.5);
}

fn turned(uv: vec2<f32>, radians: f32) -> vec2<f32> {
    let centred = (uv - vec2<f32>(0.5)) * vec2<f32>(join.aspect, 1.0);
    let c = cos(radians);
    let s = sin(radians);
    let moved = vec2<f32>(centred.x * c - centred.y * s, centred.x * s + centred.y * c);
    return moved / vec2<f32>(join.aspect, 1.0) + vec2<f32>(0.5);
}

fn ease(t: f32) -> f32 {
    return t * t * (3.0 - 2.0 * t);
}

fn black(colour: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(colour.rgb + vec3<f32>(0.0) * (1.0 - colour.a), 1.0);
}

// A heart, with values below 0 inside it.
fn heart(p: vec2<f32>) -> f32 {
    let x = p.x;
    let y = -p.y + 0.15;
    let shape = x * x + y * y - 0.3;
    return shape * shape * shape - x * x * y * y * y;
}

fn fade_through(uv: vec2<f32>, p: f32, colour: vec3<f32>) -> vec4<f32> {
    if p < 0.5 {
        let shown = a(uv);
        return vec4<f32>(mix(shown.rgb, colour, p * 2.0), mix(shown.a, 1.0, p * 2.0));
    }
    let shown = b(uv);
    return vec4<f32>(mix(colour, shown.rgb, p * 2.0 - 1.0), mix(1.0, shown.a, p * 2.0 - 1.0));
}

fn push(uv: vec2<f32>, p: f32, way: vec2<f32>) -> vec4<f32> {
    let eased = ease(p);
    let going_at = uv - way * eased;
    if inside(going_at) {
        return a(going_at);
    }
    return b(uv + way * (1.0 - eased));
}

@fragment
fn fragment_main(corner: Corner) -> @location(0) vec4<f32> {
    let uv = corner.spot;
    let p = join.progress;
    let hard = 0.002;
    let soft = 0.14;
    let peak = sin(p * PI);
    let centred = (uv - vec2<f32>(0.5)) * vec2<f32>(join.aspect, 1.0);
    switch join.kind {
        // Fades and blurs.
        case 0u: { return between(uv, p); }
        case 1u: {
            let reach = peak * 0.05;
            return mix(blurred(uv, reach, false), blurred(uv, reach, true), p);
        }
        case 2u: {
            let shown = between(uv, ease(p));
            let glow = vec3<f32>(1.0, 0.82, 0.62) * peak * peak * (0.25 + noise(uv * 3.0) * 0.5);
            return vec4<f32>(min(shown.rgb + glow * shown.a, vec3<f32>(1.0)), shown.a);
        }
        case 3u: { return fade_through(uv, p, vec3<f32>(0.0)); }
        case 4u: { return fade_through(uv, p, vec3<f32>(1.0)); }
        case 5u: {
            let band = floor(uv.y * 24.0);
            let delay = hash(vec2<f32>(band, 2.0)) * 0.5;
            let amount = clamp((p - delay) / 0.5, 0.0, 1.0);
            let slid = uv + vec2<f32>((hash(vec2<f32>(band, 9.0)) - 0.5) * peak * 0.12, 0.0);
            return mix(a(clamp(slid, vec2<f32>(0.0), vec2<f32>(1.0))),
                b(clamp(slid, vec2<f32>(0.0), vec2<f32>(1.0))), amount);
        }
        case 6u: {
            let cell = floor(uv * vec2<f32>(10.0, 6.0));
            let delay = hash(cell) * 0.75;
            return between(uv, (p - delay) / 0.25);
        }
        // Wipes, hard and soft.
        case 7u: { return between(uv, crossed(p * (1.0 + hard * 2.0) - hard - uv.y, hard)); }
        case 8u: { return between(uv, crossed(uv.y - (1.0 - p * (1.0 + hard * 2.0) + hard), hard)); }
        case 9u: { return between(uv, crossed(uv.x - (1.0 - p * (1.0 + hard * 2.0) + hard), hard)); }
        case 10u: { return between(uv, crossed(p * (1.0 + hard * 2.0) - hard - uv.x, hard)); }
        case 11u: { return between(uv, crossed(p * (1.0 + soft * 2.0) - soft - uv.y, soft)); }
        case 12u: { return between(uv, crossed(uv.y - (1.0 - p * (1.0 + soft * 2.0) + soft), soft)); }
        case 13u: { return between(uv, crossed(uv.x - (1.0 - p * (1.0 + soft * 2.0) + soft), soft)); }
        case 14u: { return between(uv, crossed(p * (1.0 + soft * 2.0) - soft - uv.x, soft)); }
        case 15u: {
            let along = (uv.x + uv.y) / 2.0;
            return between(uv, crossed(p * (1.0 + soft * 2.0) - soft - along, soft));
        }
        case 16u: {
            let strip = fract(uv.x * 8.0);
            return between(uv, crossed(p * 1.02 - strip, hard * 4.0));
        }
        case 17u: { return between(uv, crossed(p * 0.51 - abs(uv.x - 0.5), hard)); }
        case 18u: { return between(uv, crossed(p * 0.51 - abs(uv.y - 0.5), hard)); }
        case 19u: {
            let far = length(vec2<f32>(join.aspect, 1.0)) / 2.0;
            return between(uv, crossed(p * far * 1.01 - length(centred), hard));
        }
        case 20u: {
            let slanted = abs(uv.x - 0.5 + (uv.y - 0.5) * 0.3);
            return between(uv, crossed(p * 0.68 - slanted, hard));
        }
        case 21u: {
            let far = abs(uv.x - 0.5) * 0.7 + abs(uv.y - 0.5) * 1.3;
            return between(uv, crossed(p * 1.01 - far, hard));
        }
        case 22u: {
            let far = abs(uv.x - 0.5) * 1.3 + abs(uv.y - 0.5) * 0.7;
            return between(uv, crossed(p * 1.01 - far, hard));
        }
        case 23u: {
            let part = floor(uv.x * 3.0);
            let amount = clamp((p - part * 0.22) / 0.56, 0.0, 1.0);
            let rising = select(uv.y, 1.0 - uv.y, part == 1.0);
            return between(uv, crossed(ease(amount) * 1.01 - rising, hard));
        }
        // Zooms and spins.
        case 24u: {
            let reach = peak * 0.04;
            let shown = mix(blurred(uv, reach, false), blurred(uv, reach, true), ease(p));
            let brighter = shown.rgb * (1.0 + peak * 1.8) + vec3<f32>(0.75, 0.85, 1.0) * peak * peak;
            return vec4<f32>(min(brighter, vec3<f32>(1.0)), shown.a);
        }
        case 25u: {
            let row = floor(uv.y * 3.0);
            let way = 1.0 - 2.0 * (row % 2.0);
            let eased = ease(p);
            let at = uv.x + way * eased * 2.0;
            let wrapped = vec2<f32>(fract(at), uv.y);
            if eased < 0.5 {
                return a(wrapped);
            }
            return b(wrapped);
        }
        case 26u: {
            let eased = ease(p);
            let scale = 1.0 - peak * 0.55;
            let at = zoomed(turned(uv, eased * 2.0 * PI), scale);
            let clamped = clamp(at, vec2<f32>(0.0), vec2<f32>(1.0));
            return vec4<f32>(between(clamped, (p - 0.4) / 0.2).rgb, 1.0);
        }
        case 27u, 28u: {
            let inward = join.kind == 27u;
            let eased = ease(p);
            let growing = select(1.0 - eased * 0.6, 1.0 + eased * 2.0, inward);
            let arriving = select(1.6 - eased * 0.6, 0.4 + eased * 0.6, inward);
            let mid = clamp((p - 0.35) / 0.3, 0.0, 1.0);
            var total = vec4<f32>(0.0);
            let steps = 6;
            for (var index = 0; index < steps; index += 1) {
                let smear = 1.0 + f32(index) * 0.012 * peak;
                let first = zoomed(uv, growing * smear);
                let second = zoomed(uv, arriving * smear);
                total += mix(a(clamp(first, vec2<f32>(0.0), vec2<f32>(1.0))),
                    b(clamp(second, vec2<f32>(0.0), vec2<f32>(1.0))), mid);
            }
            return total / f32(steps);
        }
        // Pushes.
        case 29u: { return push(uv, p, vec2<f32>(0.0, 1.0)); }
        case 30u: { return push(uv, p, vec2<f32>(0.0, -1.0)); }
        case 31u: { return push(uv, p, vec2<f32>(-1.0, 0.0)); }
        case 32u: { return push(uv, p, vec2<f32>(1.0, 0.0)); }
        // Cartoon.
        case 33u: {
            let angle = (atan2(centred.y, centred.x) + PI) / (2.0 * PI);
            let wedge = fract(angle * 18.0);
            let centre = abs(wedge - 0.5) * 2.0;
            return between(uv, crossed(p * 1.02 - centre, 0.02));
        }
        case 34u: {
            let size = 0.02 + p * p * 3.4;
            let inside_heart = heart(centred / size * 1.1);
            return between(uv, crossed(-inside_heart, 0.0008));
        }
        case 35u: {
            let cloud = clouds(uv * vec2<f32>(join.aspect * 3.5, 3.5));
            let edge = cloud * 0.55 + (1.0 - uv.y) * 0.45;
            return between(uv, crossed(edge - (1.03 - p * 1.06), 0.015));
        }
        case 36u: {
            let slant = uv.x * 0.8 + uv.y * 0.6;
            let across = uv.y * 0.8 - uv.x * 0.6;
            let strand = floor(across * 12.0);
            let lead = hash(vec2<f32>(strand, 4.0)) * 0.45;
            let edge = fract(across * 12.0);
            let rounded = sqrt(max(1.0 - pow(edge * 2.0 - 1.0, 2.0), 0.0)) * 0.05;
            return between(uv, crossed((p * 1.7 - lead) + rounded - slant, 0.006));
        }
        // Glitches.
        case 37u: {
            let band = floor(uv.y * 14.0);
            let beat = floor(p * 14.0);
            let jolt = (hash(vec2<f32>(band, beat)) - 0.5) * 0.25 * peak;
            let at = vec2<f32>(fract(uv.x + jolt), uv.y);
            let start = hash(vec2<f32>(band, 1.0)) * 0.55;
            let amount = clamp((p - start) / 0.45, 0.0, 1.0);
            return mix(a(at), b(at), step(0.5, amount));
        }
        case 38u: {
            let wobble = vec2<f32>(
                noise(uv * vec2<f32>(4.0, 40.0) + vec2<f32>(p * 7.0, 0.0)) - 0.5,
                noise(uv * vec2<f32>(40.0, 4.0) + vec2<f32>(0.0, p * 7.0)) - 0.5,
            ) * peak * 0.09;
            let split = vec2<f32>(peak * 0.012, 0.0);
            let amount = ease(p);
            let at = clamp(uv + wobble, vec2<f32>(0.0), vec2<f32>(1.0));
            let red = mix(a(clamp(at + split, vec2<f32>(0.0), vec2<f32>(1.0))),
                b(clamp(at + split, vec2<f32>(0.0), vec2<f32>(1.0))), amount);
            let middle = mix(a(at), b(at), amount);
            let blue = mix(a(clamp(at - split, vec2<f32>(0.0), vec2<f32>(1.0))),
                b(clamp(at - split, vec2<f32>(0.0), vec2<f32>(1.0))), amount);
            return vec4<f32>(red.r, middle.g, blue.b, middle.a);
        }
        case 39u: {
            let squares = mix(160.0, 8.0, pow(peak, 0.7));
            let grid = vec2<f32>(squares * join.aspect, squares);
            let at = (floor(uv * grid) + vec2<f32>(0.5)) / grid;
            let clamped = clamp(at, vec2<f32>(0.0), vec2<f32>(1.0));
            if p < 0.5 {
                return a(clamped);
            }
            return b(clamped);
        }
        // 3D, all with black behind.
        case 40u: {
            let eased = ease(p);
            let seam = 1.0 - eased;
            let lean = peak * 0.22;
            if uv.x < seam {
                let along = uv.x / max(seam, 0.001);
                let height = 1.0 - lean * (1.0 - along);
                let at = vec2<f32>(along, (uv.y - 0.5) / height + 0.5);
                return black(a(at));
            }
            let along = (uv.x - seam) / max(eased, 0.001);
            let height = 1.0 - lean * along;
            let at = vec2<f32>(along, (uv.y - 0.5) / height + 0.5);
            return black(b(at));
        }
        case 41u: {
            let eased = ease(p);
            let scale = 1.0 - peak * 0.35;
            let first = zoomed(uv, scale) + vec2<f32>(eased * 1.25, 0.0);
            let second = zoomed(uv, scale) - vec2<f32>((1.0 - eased) * 1.25, 0.0);
            if inside(first) {
                return black(a(first));
            }
            return black(b(second));
        }
        case 42u: {
            // The page folds along a slanted line that travels from right to left.
            let fold = 1.3 - ease(p) * 1.7 + (uv.y - 0.5) * 0.25;
            let past = uv.x - fold;
            if past < 0.0 {
                let at = uv;
                let shade = 1.0 - smoothstep(-0.12, 0.0, past) * 0.25;
                return vec4<f32>(a(at).rgb * shade, a(at).a);
            }
            let flap = 0.2;
            if past < flap {
                let curl = sin(past / flap * PI * 0.5) * flap;
                let back = vec2<f32>(fold - curl, uv.y);
                let paper = mix(a(back).rgb, vec3<f32>(1.0), 0.75);
                return vec4<f32>(paper * (0.8 + past / flap * 0.2), 1.0);
            }
            return b(uv);
        }
        default: {
            // The old picture draws back between panels that open to show the new one.
            let eased = ease(p);
            if p < 0.5 {
                let scale = 1.0 - eased * 0.7;
                return black(a(zoomed(uv, scale)));
            }
            let scale = 0.3 + (eased - 0.5) * 1.4;
            return black(b(zoomed(uv, scale)));
        }
    }
    return vec4<f32>(0.0);
}
