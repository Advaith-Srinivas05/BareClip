"""Measures each filter from a recording and writes crates/bareclip-core/src/filter_grades.rs.

Usage: python scripts/fit-filters.py
Needs numpy, scipy and the FFmpeg in vendor/ffmpeg/bin.

The recording, design/plan/images/filter/filter.mp4, plays the same clip once with no filter and
then once with each filter, 5 seconds each, in the order of RECORDED. Because the clip starts again
every 5 seconds, each filtered frame can be laid over the same frame without a filter, and the
colours fitted from one to the other. Filters the recording doesn't have keep BareClip's own looks,
from OWN, which are fitted from the formula they used before.

Every filter becomes the same three steps, which the shader runs: a mix of red, green and blue
with something added, then a curve on each colour through nine evenly spaced points.
"""

import subprocess
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares

ROOT = Path(__file__).resolve().parent.parent
RECORDING = ROOT / "design" / "plan" / "images" / "filter" / "filter.mp4"
FFMPEG = ROOT / "vendor" / "ffmpeg" / "bin" / "ffmpeg.exe"
OUTPUT = ROOT / "crates" / "bareclip-core" / "src" / "filter_grades.rs"

WIDTH, HEIGHT = 320, 180
FRAMES_PER_SLOT = 150
SAMPLES_PER_SLOT = 15
POINTS = 9
LUMA = np.array([0.2126, 0.7152, 0.0722])

# In the recording's order, after the slot with no filter. Names are the Rust variants.
RECORDED = [
    "OrangeAndTeal", "BoldAndBlue", "GoldenHour", "VibrantVlogger", "PurpleUndertone",
    "Film35mm", "Contrast", "Autumn", "Winter", "OldWestern", "WarmCoastline", "CoolCoastline",
    "Golden", "WarmToneFilm", "Increased", "Scenery", "Portrait", "Indoors", "Outdoors", "Muted",
    "BlackAndWhite1", "BlackAndWhite2", "MutedBlackAndWhite", "Gloomy", "DeepFried", "Euphoric",
    "YellowOrangeDuotone", "PinkPurpleDuotone", "BluePinkDuotone", "GreenBlueDuotone",
    "WhiteOverlay", "BlackOverlay", "YellowOverlay", "OrangeOverlay", "RedOverlay",
    "PinkOverlay", "PurpleOverlay", "BlueOverlay", "TurquoiseOverlay", "GreenOverlay",
]

# BareClip's own looks for the filters the recording can't show: exposure, contrast, saturation
# and temperature, then what shadows and highlights are multiplied by.
WARM_SHADOWS = [1.05, 0.98, 0.9]
COOL_SHADOWS = [0.9, 0.98, 1.08]
OWN = {
    "Retro": ([0.05, -0.1, -0.2, 0.25], [1.1, 1.0, 0.85], [1.02, 0.98, 0.85]),
    "WinterSunset": ([0.0, 0.08, 0.05, 0.1], [0.9, 0.9, 1.2], [1.12, 0.98, 0.9]),
    "WarmCountryside": ([0.03, 0.05, 0.15, 0.2], WARM_SHADOWS, [1.03, 1.02, 0.92]),
    "CoolCountryside": ([0.03, 0.05, 0.12, -0.2], COOL_SHADOWS, [0.97, 1.02, 1.03]),
    "Dreamscape": ([0.1, -0.2, 0.15, -0.05], [1.05, 0.92, 1.15], [1.03, 1.0, 1.05]),
    "Sunrise": ([0.1, 0.0, 0.12, 0.3], [1.05, 0.92, 0.95], [1.1, 1.0, 0.86]),
    "CoolTone": ([0.0, 0.05, -0.05, -0.32], [1.0] * 3, [1.0] * 3),
    "PastelDreams": ([0.14, -0.3, -0.15, 0.05], [1.12, 1.02, 1.12], [1.02, 1.0, 1.02]),
    "SoftBlackAndWhite": ([0.08, -0.2, -1.0, 0.0], [1.0] * 3, [1.0] * 3),
}


def recorded_frames():
    """Small frames from each slot, as colours from 0 to 1, by slot."""
    step = FRAMES_PER_SLOT // SAMPLES_PER_SLOT
    pick = f"eq(mod(mod(n\\,{FRAMES_PER_SLOT})\\,{step})\\,{step // 2})"
    raw = subprocess.run(
        [
            str(FFMPEG), "-v", "error", "-i", str(RECORDING), "-vf",
            f"select='{pick}',scale={WIDTH}:{HEIGHT}:in_color_matrix=bt709:in_range=tv:flags=area,"
            "format=rgb24",
            "-fps_mode", "passthrough", "-f", "rawvideo", "-",
        ],
        check=True,
        capture_output=True,
    ).stdout
    frames = np.frombuffer(raw, dtype=np.uint8).astype(np.float64) / 255
    slots = len(RECORDED) + 1
    return frames[: slots * SAMPLES_PER_SLOT * HEIGHT * WIDTH * 3].reshape(slots, -1, 3)


def curve(values, points):
    spot = np.clip(values, 0, 1) * (POINTS - 1)
    index = np.minimum(spot.astype(int), POINTS - 2)
    along = spot - index
    return points[index] * (1 - along) + points[index + 1] * along


def apply(params, colours):
    mix = params[:12].reshape(3, 4)
    points = params[12:].reshape(3, POINTS)
    mixed = colours @ mix[:, :3].T + mix[:, 3]
    return np.stack([curve(mixed[:, c], points[c]) for c in range(3)], -1)


def fit(before, after):
    """The closest mix and curves, starting from a straight-line fit and from one on brightness,
    because a gradient from dark to light is far from a straight line in the colours."""
    ones = np.c_[before, np.ones(len(before))]
    straight, *_ = np.linalg.lstsq(ones, after, rcond=None)
    identity = np.linspace(0, 1, POINTS)
    starts = [np.r_[straight.T.ravel(), np.tile(identity, 3)]]
    bins = np.clip((before @ LUMA * (POINTS - 1)).round().astype(int), 0, POINTS - 1)
    by_brightness = [
        [after[bins == i, c].mean() if (bins == i).any() else identity[i] for i in range(POINTS)]
        for c in range(3)
    ]
    luma_mix = np.c_[np.tile(LUMA, (3, 1)), np.zeros(3)]
    starts.append(np.r_[luma_mix.ravel(), np.ravel(by_brightness)])

    def misses(params):
        bends = np.diff(params[12:].reshape(3, POINTS), 2, axis=1).ravel()
        return np.r_[(apply(params, before) - after).ravel(), 0.02 * bends]

    best = None
    for start in starts:
        found = least_squares(misses, start, loss="soft_l1", f_scale=0.05, max_nfev=300).x
        error = np.abs(apply(found, before) - after).mean()
        if best is None or error < best[0]:
            best = (error, found)
    return best


def old_formula(colours, basic, shadows, highlights):
    """What BareClip's filters did before they were measured, for the ones that keep that look."""

    def basic_grade(rgb, values):
        exposure, contrast, saturation, temperature = values
        rgb = rgb * 2 ** (exposure * 1.5)
        rgb = (rgb - 0.5) * (1 + contrast) + 0.5
        grey = (rgb @ LUMA)[:, None]
        rgb = grey + (rgb - grey) * (1 + saturation)
        return rgb + np.array([temperature * 0.15, 0, -temperature * 0.15])

    rgb = np.clip(basic_grade(colours, basic), 0, 1)
    luma = (rgb @ LUMA)[:, None]
    rgb = rgb * (np.array(shadows) * (1 - luma) + np.array(highlights) * luma)
    return np.clip(rgb, 0, 1)


def rust(name, params):
    mix = params[:12].reshape(3, 4)
    points = params[12:].reshape(3, POINTS)
    number = lambda value: f"{value:.4f}"
    rows = ", ".join("[" + ", ".join(map(number, row)) + "]" for row in mix)
    curves = ", ".join("[" + ", ".join(map(number, row)) + "]" for row in points)
    return f"        Filter::{name} => FilterGrade {{\n            mix: [{rows}],\n            curves: [{curves}],\n        }},\n"


def main():
    rng = np.random.default_rng(0)
    slots = recorded_frames()
    plain = slots[0]
    pick = rng.choice(len(plain), 12000, replace=False)
    fitted = {}
    for slot, name in enumerate(RECORDED, start=1):
        error, params = fit(plain[pick], slots[slot][pick])
        whole = np.abs(apply(params, plain) - slots[slot]).mean() * 255
        print(f"{name:22s} {whole:5.2f} levels out of 255 on average")
        fitted[name] = params
    grid = np.stack(np.meshgrid(*[np.linspace(0, 1, 17)] * 3, indexing="ij"), -1).reshape(-1, 3)
    for name, (basic, shadows, highlights) in OWN.items():
        error, params = fit(grid, old_formula(grid, basic, shadows, highlights))
        print(f"{name:22s} {error * 255:5.2f} levels from its old look")
        fitted[name] = params

    body = "".join(rust(name, fitted[name]) for name in RECORDED + list(OWN))
    OUTPUT.write_text(
        "//! Made by `scripts/fit-filters.py`, so don't change it by hand.\n"
        "\n"
        "use super::{Filter, FilterGrade};\n"
        "\n"
        "/// What each filter does at its usual strength. `None` changes nothing.\n"
        "#[allow(clippy::too_many_lines, clippy::excessive_precision)]\n"
        "pub(super) fn filter_grade(filter: Filter) -> FilterGrade {\n"
        "    match filter {\n"
        "        Filter::None => FilterGrade::NONE,\n"
        f"{body}"
        "    }\n"
        "}\n",
        encoding="utf-8",
        newline="\n",
    )


if __name__ == "__main__":
    main()
