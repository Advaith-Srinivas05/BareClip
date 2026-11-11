"""Generates every BareClip brand asset from brand_geometry.py and tokens.json.

Usage: python scripts/build-brand.py --resvg path/to/resvg
Needs Pillow and the resvg command-line renderer.
"""

import argparse
import json
import math
import subprocess
from pathlib import Path

from PIL import Image

from brand_geometry import MARK_BOX, WORD_BOX, mark_group, small_mark_group, word_group

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
SVG = BRAND / "svg"
PNG = BRAND / "png"
WINDOWS = BRAND / "windows"
TOKENS = json.loads((BRAND / "tokens.json").read_text())
LOGO = TOKENS["logo"]
FONT_FILE = ROOT / "assets" / "fonts" / "HankenGrotesk-Variable.ttf"
FONT = "'Hanken Grotesk', sans-serif"


def svg(width, height, body, title, view_box=None):
    box = view_box or f"0 0 {width} {height}"
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="{box}" role="img"><title>{title}</title>{body}</svg>\n'
    )


def write(name, text):
    (SVG / f"{name}.svg").write_text(text, encoding="utf-8")


def placed(group, box, x, y, scale):
    """Places artwork so the top-left of its box lands on (x, y)."""
    tx, ty = x - box[0] * scale, y - box[1] * scale
    return f'<g transform="translate({tx:.3f} {ty:.3f}) scale({scale:.5f})">{group}</g>'


def full_mark(theme):
    if theme == "dark":
        return mark_group(LOGO["charcoal"], LOGO["bronze"], LOGO["bronze"], LOGO["black"])
    return mark_group(LOGO["charcoal"], LOGO["bronze"], LOGO["bronze"])


def small_mark(theme):
    if theme == "dark":
        return small_mark_group(LOGO["bronze"], LOGO["gold"], LOGO["gold"])
    return small_mark_group(LOGO["charcoal"], LOGO["bronze"], LOGO["bronze"])


def word_color(theme):
    return LOGO["gold"] if theme == "dark" else LOGO["bronze"]


def build_marks():
    x, y, w, h = MARK_BOX
    view = f"{x} {y} {w} {h}"
    for theme in ("dark", "light"):
        write(f"mark-{theme}", svg(900, 1044, full_mark(theme), "BareClip mark", view))
        write(f"mark-small-{theme}", svg(900, 1044, small_mark(theme), "BareClip small mark", view))
    for name, color in (("black", "#000000"), ("white", "#FFFFFF"), ("gold", LOGO["gold"])):
        write(f"mark-mono-{name}", svg(900, 1044, mark_group(color, color), "BareClip mark", view))


def build_wordmarks():
    x, y, w, h = WORD_BOX
    view = f"{x} {y} {w} {h}"
    colors = {"gold": LOGO["gold"], "bronze": LOGO["bronze"], "black": "#000000", "white": "#FFFFFF"}
    for name, color in colors.items():
        write(f"wordmark-{name}", svg(1280, 153, word_group(color), "BareClip", view))


def build_lockups():
    for theme in ("dark", "light"):
        word = word_group(word_color(theme))
        # Vertical keeps the proportions of the original logo.
        mark_scale, word_scale = 0.2, 0.25
        mark_w, mark_h = MARK_BOX[2] * mark_scale, MARK_BOX[3] * mark_scale
        word_w = WORD_BOX[2] * word_scale
        body = placed(full_mark(theme), MARK_BOX, (word_w - mark_w) / 2, 0, mark_scale)
        body += placed(word, WORD_BOX, 0, mark_h + 27, word_scale)
        height = mark_h + 27 + WORD_BOX[3] * word_scale
        write(f"lockup-vertical-{theme}", svg(320, round(height), body, "BareClip"))

        mark_scale = 120 / MARK_BOX[3]
        word_scale = 0.26
        mark_w = MARK_BOX[2] * mark_scale
        word_h = WORD_BOX[3] * word_scale
        width = mark_w + 28 + WORD_BOX[2] * word_scale
        body = placed(full_mark(theme), MARK_BOX, 0, 0, mark_scale)
        body += placed(word, WORD_BOX, mark_w + 28, (120 - word_h) / 2, word_scale)
        write(f"lockup-horizontal-{theme}", svg(round(width), 120, body, "BareClip"))


def icon_body(group, mark_height, radius):
    scale = mark_height / MARK_BOX[3]
    x = (1024 - MARK_BOX[2] * scale) / 2
    y = (1024 - mark_height) / 2
    tile = f'<rect width="1024" height="1024" rx="{radius}" fill="{LOGO["ink"]}"/>'
    return tile + placed(group, MARK_BOX, x, y, scale)


def build_icons():
    write("app-icon", svg(1024, 1024, icon_body(full_mark("dark"), 800, 224), "BareClip"))
    write("app-icon-small", svg(1024, 1024, icon_body(small_mark("dark"), 860, 190), "BareClip"))


def build_pattern():
    width, height, band = 120, 120 * math.tan(math.radians(30)), 16
    for name, color in (("bronze", LOGO["bronze"]), ("gold", LOGO["gold"])):
        body = ""
        for shift in (0, -height):
            points = [(0, height + shift), (width, shift), (width, band + shift), (0, height + band + shift)]
            body += f'<polygon fill="{color}" points="{" ".join(f"{a:.2f},{b:.2f}" for a, b in points)}"/>'
        write(f"pattern-slant-{name}", svg(width, round(height, 2), body, "BareClip slant pattern"))


def build_social():
    scale = 980 / MARK_BOX[3]
    body = f'<rect width="1280" height="640" fill="{LOGO["ink"]}"/>'
    body += placed(full_mark("dark"), MARK_BOX, 670, -170, scale)
    body += placed(word_group(LOGO["gold"]), WORD_BOX, 84, 262, 0.42)
    body += (
        f'<text x="84" y="382" font-family="{FONT}" font-size="30" '
        f'fill="{TOKENS["dark"]["text_muted"]}">Fast, offline video editing for Windows.</text>'
    )
    write("social-preview", svg(1280, 640, body, "BareClip: fast, offline video editing for Windows"))


def build_installer():
    # WiX draws black title text on the left of the banner, so that side stays white.
    scale = 44 / MARK_BOX[3]
    x = 493 - MARK_BOX[2] * scale - 12
    banner = '<rect width="493" height="58" fill="#FFFFFF"/>' + placed(small_mark("light"), MARK_BOX, x, 7, scale)
    write("msi-banner", svg(493, 58, banner, "BareClip installer banner"))

    # WiX draws black dialog text from x=164 onward, so only the left panel carries artwork.
    scale = 112 / MARK_BOX[2]
    dialog = '<rect width="493" height="312" fill="#FFFFFF"/>'
    dialog += f'<rect width="164" height="312" fill="{LOGO["ink"]}"/>'
    dialog += placed(full_mark("dark"), MARK_BOX, 26, 76, scale)
    dialog += placed(word_group(LOGO["gold"]), WORD_BOX, 26, 222, 112 / WORD_BOX[2])
    write("msi-dialog", svg(493, 312, dialog, "BareClip installer dialog"))


def text(x, y, value, fill, size=13, weight=400, anchor="start"):
    return (
        f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="{size}" font-weight="{weight}" '
        f'text-anchor="{anchor}" fill="{fill}">{value}</text>'
    )


def rect(x, y, w, h, fill, radius=0, stroke=None):
    border = f' stroke="{stroke}"' if stroke else ""
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill}"{border}/>'


def clip(x, y, w, h, fill, label, label_fill, selected=None, wave=False):
    """A timeline clip, with opposite corners cut at the logo's 30 degree angle."""
    cut_x, cut_y = 10, 5.8
    points = [(x + cut_x, y), (x + w, y), (x + w, y + h - cut_y), (x + w - cut_x, y + h), (x, y + h), (x, y + cut_y)]
    outline = f' stroke="{selected}" stroke-width="2"' if selected else ""
    out = f'<polygon fill="{fill}"{outline} points="{" ".join(f"{a},{b}" for a, b in points)}"/>'
    if wave:
        mid = y + h - 12
        for bar in range(int(x) + 8, int(x + w) - 6, 4):
            amp = 1.5 + 7 * abs(math.sin(bar * 0.19) * math.cos(bar * 0.043))
            out += f'<rect x="{bar}" y="{mid - amp:.1f}" width="2" height="{amp * 2:.1f}" fill="{label_fill}" opacity=".55"/>'
    out += text(x + 14, y + 17, label, label_fill, 12, 500)
    if selected:
        out += rect(x - 1, y + 9, 5, h - 18, selected, 2) + rect(x + w - 4, y + 9, 5, h - 18, selected, 2)
    return out


def build_theme_preview(theme):
    t = TOKENS[theme]
    width, height = 1280, 760
    top, left, right, timeline = 48, 256, 264, 268
    bottom = height - timeline
    b = rect(0, 0, width, height, t["bg_app"])

    # Top bar
    b += rect(0, 0, width, top, t["surface"]) + rect(0, top - 1, width, 1, t["border"])
    scale = 26 / MARK_BOX[3]
    b += placed(small_mark(theme), MARK_BOX, 16, 11, scale)
    b += text(50, 29, "Gameplay highlights", t["text"], 14, 600)
    b += rect(width - 112, 9, 96, 30, t["accent"], 5)
    b += text(width - 64, 29, "Export", t["accent_text"], 13, 600, "middle")
    b += text(width - 128, 29, "Saved", t["text_muted"], 13, 400, "end")

    # Media panel
    b += rect(0, top, left, bottom - top, t["surface"]) + rect(left - 1, top, 1, bottom - top, t["border"])
    b += text(16, top + 30, "Media", t["text"], 14, 600)
    files = [("ranked-match.mkv", "12:04"), ("mic-commentary.wav", "11:58"), ("menu-capture.mp4", "02:17")]
    for index, (name, length) in enumerate(files):
        y = top + 48 + index * 66
        if index == 0:
            b += rect(8, y - 6, left - 17, 62, t["selection"], 5)
        name_fill = t["selection_text"] if index == 0 else t["text"]
        meta_fill = t["selection_text"] if index == 0 else t["text_muted"]
        b += rect(16, y, 76, 50, f"url(#scene-{theme})", 3)
        b += text(102, y + 21, name, name_fill, 12, 500) + text(102, y + 39, length, meta_fill, 12)

    # Stage. This picture of the interface predates the surround following the theme.
    stage_w = width - left - right
    stage_h = bottom - top
    b += rect(left, top, stage_w, stage_h, t["stage"])
    frame_w = stage_w - 92
    frame_h = frame_w * 9 / 16
    frame_x = left + 46
    frame_y = top + 16
    gradient = f"scene-{theme}"
    b += (
        f'<defs><linearGradient id="{gradient}" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="#3A4048"/><stop offset=".55" stop-color="#6A5A48"/>'
        f'<stop offset="1" stop-color="#15130F"/></linearGradient></defs>'
    )
    b += rect(frame_x, frame_y, frame_w, frame_h, f"url(#{gradient})")
    b += text(frame_x + frame_w / 2, frame_y + frame_h - 30, "Final round", "#FFFFFF", 26, 600, "middle")
    transport_y = frame_y + frame_h + 26
    centre = left + stage_w / 2
    on_stage = "#F1EEE9"
    b += text(frame_x, transport_y + 5, "00:00:55.17", on_stage, 15, 600)
    b += text(frame_x + frame_w, transport_y + 5, "00:12:04.00", "#ABA69F", 13, 400, "end")
    b += f'<circle cx="{centre}" cy="{transport_y}" r="17" fill="{LOGO["gold"]}"/>'
    b += f'<polygon fill="{LOGO["ink"]}" points="{centre - 5},{transport_y - 8} {centre + 8},{transport_y} {centre - 5},{transport_y + 8}"/>'

    # Inspector
    ix = width - right
    b += rect(ix, top, right, bottom - top, t["surface"]) + rect(ix, top, 1, bottom - top, t["border"])
    b += text(ix + 16, top + 30, "Clip", t["text"], 14, 600)
    rows = [("Scale", "100%", 1.0), ("Opacity", "85%", 0.85), ("Speed", "1.5x", 0.5), ("Volume", "-3 dB", 0.72)]
    for index, (label, value, amount) in enumerate(rows):
        y = top + 62 + index * 58
        b += text(ix + 16, y, label, t["text_muted"], 12) + text(width - 16, y, value, t["text"], 12, 500, "end")
        b += rect(ix + 16, y + 12, right - 32, 4, t["border_strong"], 2)
        b += rect(ix + 16, y + 12, (right - 32) * amount, 4, t["accent"], 2)
        knob = ix + 16 + (right - 32) * amount
        b += f'<circle cx="{knob:.1f}" cy="{y + 14}" r="6" fill="{t["accent"]}" stroke="{t["raised"]}" stroke-width="2"/>'

    # Timeline
    b += rect(0, bottom, width, 1, t["border"])
    b += rect(0, bottom + 1, 88, timeline - 1, t["surface"])
    for second in range(0, 12):
        x = 88 + second * 98
        b += rect(x, bottom + 8, 1, 22, t["border"])
        b += text(x + 7, bottom + 23, f"0{second // 6}:{second % 6 * 10:02d}", t["text_muted"], 11)
    tracks = [("Title", 34), ("Video", 44), ("Game", 44), ("Mic", 44)]
    y = bottom + 34
    positions = []
    for name, track_h in tracks:
        positions.append(y)
        b += text(14, y + track_h / 2 + 4, name, t["text_muted"], 12, 500)
        b += rect(88, y + track_h + 3, width - 88, 1, t["border"])
        y += track_h + 8
    b += clip(392, positions[0], 186, 34, t["clip_overlay"], "Final round", t["clip_overlay_text"])
    b += clip(92, positions[1], 300, 44, t["clip_video"], "ranked-match", t["clip_video_text"])
    b += clip(396, positions[1], 470, 44, t["clip_video"], "ranked-match", t["clip_video_text"], selected=t["text"])
    b += clip(870, positions[1], 290, 44, t["clip_video"], "menu-capture", t["clip_video_text"])
    b += clip(92, positions[2], 300, 44, t["clip_audio"], "Game audio", t["clip_audio_text"], wave=True)
    b += clip(396, positions[2], 470, 44, t["clip_audio"], "Game audio", t["clip_audio_text"], wave=True)
    b += clip(92, positions[3], 774, 44, t["clip_audio"], "Mic commentary", t["clip_audio_text"], wave=True)

    # Playhead, with a head taken from the mark's hexagon
    px = 634
    b += rect(px - 2, bottom + 6, 4, timeline - 18, t["playhead_outline"])
    b += rect(px - 1, bottom + 6, 2, timeline - 18, t["playhead"])
    head_top = bottom + 3
    b += f'<polygon fill="{t["playhead"]}" stroke="{t["playhead_outline"]}" points="{px},{head_top} {px + 7},{head_top + 4} {px + 7},{head_top + 12} {px},{head_top + 16} {px - 7},{head_top + 12} {px - 7},{head_top + 4}"/>'
    write(f"theme-preview-{theme}", svg(width, height, b, f"BareClip editor, {theme} theme"))


def build_palette():
    logo_values = {v.upper() for v in LOGO.values()}
    roles = [
        ("bg_app", "App background"), ("surface", "Panels"), ("raised", "Raised controls"),
        ("border", "Dividers"), ("border_strong", "Control outlines"), ("text", "Text"),
        ("text_muted", "Secondary text"), ("accent", "Accent"), ("accent_hover", "Accent hover"),
        ("selection", "Selection"), ("playhead", "Playhead"), ("stage", "Video stage"), ("clip_video", "Video clips"),
        ("clip_audio", "Audio clips"), ("clip_overlay", "Overlay clips"), ("clip_shape", "Library clips"),
        ("danger", "Errors"),
        ("success", "Success"),
    ]
    width, row, head = 1000, 44, 76
    height = head + row * len(roles) + 20
    body = ""
    for column, theme in enumerate(("dark", "light")):
        t = TOKENS[theme]
        x = column * width / 2
        body += rect(x, 0, width / 2, height, t["surface"])
        body += text(x + 28, 46, f"{theme.capitalize()} theme", t["text"], 22, 600)
        for index, (key, label) in enumerate(roles):
            y = head + index * row
            value = t[key].upper()
            body += rect(x + 28, y + 5, 56, 32, t[key], 4, t["border"])
            body += text(x + 100, y + 26, label, t["text"], 14, 500)
            suffix = "  (logo)" if value in logo_values else ""
            body += text(x + width / 2 - 28, y + 26, value + suffix, t["text_muted"], 13, 400, "end")
    write("palette", svg(width, height, body, "BareClip colour palette"))


def render(resvg, name, out, width=None):
    command = [resvg, "--use-font-file", str(FONT_FILE), str(SVG / f"{name}.svg"), str(out)]
    if width:
        command[1:1] = ["-w", str(width)]
    subprocess.run(command, check=True)


def build_exports(resvg):
    for name in ("mark-dark", "mark-light", "mark-mono-black", "mark-mono-white", "mark-mono-gold"):
        for size in (256, 512, 1024):
            render(resvg, name, PNG / f"{name}-{size}.png", size)
    for name in ("mark-small-dark", "mark-small-light"):
        render(resvg, name, PNG / f"{name}-256.png", 256)
    for name in ("wordmark-gold", "wordmark-bronze", "wordmark-black", "wordmark-white"):
        render(resvg, name, PNG / f"{name}-1280.png", 1280)
    for theme in ("dark", "light"):
        render(resvg, f"lockup-vertical-{theme}", PNG / f"lockup-vertical-{theme}-1024.png", 1024)
        render(resvg, f"lockup-horizontal-{theme}", PNG / f"lockup-horizontal-{theme}-1600.png", 1600)
        render(resvg, f"theme-preview-{theme}", PNG / f"theme-preview-{theme}.png", 1920)
    render(resvg, "palette", PNG / "palette.png", 2000)
    render(resvg, "social-preview", PNG / "social-preview-1280x640.png")

    # The detailed mark turns to mush at small sizes, so these use the simplified one.
    icon_sizes = {size: "app-icon-small" for size in (16, 20, 24, 32, 40, 48, 64)}
    icon_sizes.update({size: "app-icon" for size in (128, 256, 512, 1024)})
    for size, name in icon_sizes.items():
        render(resvg, name, PNG / f"app-icon-{size}.png", size)

    ico_sizes = [16, 20, 24, 32, 40, 48, 64, 256]
    frames = [Image.open(PNG / f"app-icon-{size}.png").convert("RGBA") for size in ico_sizes]
    frames[-1].save(
        WINDOWS / "bareclip.ico",
        format="ICO",
        sizes=[(size, size) for size in ico_sizes],
        append_images=frames[:-1],
    )

    for name, size in (("msi-banner", (493, 58)), ("msi-dialog", (493, 312))):
        png = WINDOWS / f"{name}.png"
        render(resvg, name, png)
        image = Image.open(png).convert("RGB")
        assert image.size == size, f"{name} is {image.size}"
        image.save(WINDOWS / f"{name}.bmp")
        png.unlink()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--resvg", default="resvg")
    args = parser.parse_args()
    for folder in (SVG, PNG, WINDOWS):
        folder.mkdir(parents=True, exist_ok=True)
    build_marks()
    build_wordmarks()
    build_lockups()
    build_icons()
    build_pattern()
    build_social()
    build_installer()
    build_theme_preview("dark")
    build_theme_preview("light")
    build_palette()
    build_exports(args.resvg)
    count = sum(1 for path in BRAND.rglob("*") if path.is_file())
    print(f"Brand assets ready: {count} files in {BRAND}")


if __name__ == "__main__":
    main()
