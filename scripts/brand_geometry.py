"""Vector geometry for the BareClip logo.

Mark coordinates are 5x the mark in assets/brand/source/logo-original.png, offset
by (268, 143). Wordmark coordinates are 4x the wordmark, offset by (201, 379).
"""

import math

MARK_BOX = (30, 28, 900, 1044)
WORD_BOX = (18, 17, 1280, 153)

# A vertex is (x, y) or (x, y, corner_radius).
FRAME_OUTER = [
    (405, 30, 16), (930, 328, 30), (930, 768, 30), (405, 1072, 16),
    (30, 852, 30), (30, 247, 30),
]
FRAME_BRONZE = [
    (405, 30, 16), (930, 328, 30), (930, 768, 30), (425, 1062, 8), (425, 1005),
    (878, 745), (878, 358), (850, 345), (405, 85),
]
WINDOWS = [
    # Clapper
    [(405, 85), (850, 345), (730, 412), (285, 157)],
    # Middle facet
    [(257, 205), (675, 443), (558, 513), (257, 343)],
    # Small triangles
    [(113, 257), (203, 205), (203, 312)],
    [(285, 945), (375, 893), (375, 997)],
    [(785, 443), (878, 390), (878, 495)],
    # Left panel, around the trim bar
    [
        (82, 303), (205, 373), (205, 470), (255, 470), (255, 402), (375, 470),
        (375, 835), (255, 903), (255, 737), (205, 737), (205, 900), (82, 832),
    ],
    # Right panel, around the diagonal band. The step leaves room for the play tip.
    [
        (430, 500), (545, 567, 14), (573, 567, 14), (730, 473), (848, 544),
        (683, 647), (688, 665), (455, 800), (455, 848), (706, 703), (724, 674),
        (878, 592), (878, 745), (430, 1003),
    ],
]
ACCENTS = [
    # Top clapper stripes
    [(325, 157, 10), (365, 131, 8), (440, 131, 8), (522, 180), (360, 180, 10)],
    [(432, 217), (590, 217, 6), (668, 263), (505, 263, 6)],
    [(578, 302), (735, 302, 6), (808, 347), (655, 347, 6)],
    # Lower clapper stripes
    [(780, 672), (858, 626), (858, 672)],
    [(662, 757), (735, 712), (858, 712), (858, 732, 8), (820, 757)],
    [(515, 842), (590, 795), (748, 795), (672, 842)],
    [(450, 878), (598, 878), (520, 925), (450, 925)],
    # Trim arrow and right marker
    [(213, 498), (243, 498), (243, 650), (275, 650), (228, 713), (180, 650), (213, 650)],
    [(345, 540), (345, 622), (278, 580)],
    # Play
    [(448, 543, 16), (654, 656, 18), (448, 774, 16)],
]
FRAME_ACCENTS = [
    # Left marker uses the frame colour
    [(110, 540), (175, 580), (110, 620)],
]

# Simplified mark for small sizes.
SMALL_OUTER = [
    (405, 30, 20), (930, 328, 40), (930, 768, 40), (405, 1072, 20),
    (30, 852, 40), (30, 247, 40),
]
SMALL_RIGHT = [
    (405, 30, 20), (930, 328, 40), (930, 768, 40), (405, 1072, 20), (405, 930),
    (800, 702), (800, 395), (405, 172),
]
SMALL_INNER = [(405, 172), (800, 395), (800, 702), (405, 930), (160, 788), (160, 310)]
SMALL_PLAY = [(330, 360, 28), (690, 552, 32), (330, 742, 28)]


def fmt(value):
    text = f"{value:.2f}".rstrip("0").rstrip(".")
    return "0" if text == "-0" else text


def poly_path(points):
    """A closed path, rounding any vertex that has a corner radius."""
    vertices = [p if len(p) == 3 else (p[0], p[1], 0) for p in points]
    parts = []
    count = len(vertices)
    for index, (x, y, radius) in enumerate(vertices):
        if radius == 0:
            parts.append(("L", x, y))
            continue
        before = vertices[index - 1]
        after = vertices[(index + 1) % count]
        start = _toward((x, y), before, radius)
        end = _toward((x, y), after, radius)
        parts.append(("L", *start))
        parts.append(("Q", x, y, *end))
    first = parts[0]
    path = f"M{fmt(first[1])} {fmt(first[2])}"
    for part in parts[1:]:
        if part[0] == "L":
            path += f"L{fmt(part[1])} {fmt(part[2])}"
        else:
            path += f"Q{fmt(part[1])} {fmt(part[2])} {fmt(part[3])} {fmt(part[4])}"
    return path + "Z"


def _toward(point, other, distance):
    dx, dy = other[0] - point[0], other[1] - point[1]
    length = math.hypot(dx, dy)
    distance = min(distance, length / 2)
    return point[0] + dx / length * distance, point[1] + dy / length * distance


def _c_letter():
    cx, cy, outer, inner = 790, 93.5, 77, 41
    outer_angle, inner_angle = math.radians(36), math.radians(39)
    ox, oy = outer * math.cos(outer_angle), outer * math.sin(outer_angle)
    ix, iy = inner * math.cos(inner_angle), inner * math.sin(inner_angle)
    return (
        f"M{fmt(cx + ox)} {fmt(cy - oy)}"
        f"A{outer} {outer} 0 1 0 {fmt(cx + ox)} {fmt(cy + oy)}"
        f"L{fmt(cx + ix)} {fmt(cy + iy)}"
        f"A{inner} {inner} 0 1 1 {fmt(cx + ix)} {fmt(cy - iy)}Z"
    )


WORD_PATH = "".join([
    # B
    "M18 20H86C114 20 126 38 126 57C126 71 118 84 106 90C121 95 130 108 130 123"
    "C130 149 110 165 84 165H18ZM54 50V76H80C89 76 93 70 93 63C93 56 89 50 80 50Z"
    "M54 105V135H83C93 135 97 128 97 120C97 112 93 105 83 105Z",
    # A
    "M178 165L228 20H270L320 165H280L272 140H226L218 165ZM236 110H262L249 72Z",
    # R
    "M382 20H445C478 20 495 42 495 68C495 90 483 105 465 112L498 165H455L427 118H418V165H382Z"
    "M418 50V88H442C454 88 460 80 460 69C460 58 454 50 442 50Z",
    # E
    "M560 20H650V53H596V76H645V107H596V132H652V165H560Z",
    # C
    _c_letter(),
    # L
    "M920 20H956V132H1012V165H920Z",
    # I
    "M1077 20H1113V165H1077Z",
    # P
    "M1192 20H1250C1283 20 1298 42 1298 70C1298 98 1280 122 1250 122H1228V165H1192Z"
    "M1228 52V90H1248C1259 90 1264 82 1264 71C1264 60 1259 52 1248 52Z",
])


def mark_group(frame, accent, frame_right=None, window=None):
    """The full mark. Windows are real holes, so `window=None` leaves them transparent."""
    frame_right = frame_right or frame
    holes = "".join(poly_path(w) for w in WINDOWS)
    out = ""
    if window:
        out += f'<path fill="{window}" d="{holes}"/>'
    out += f'<path fill="{frame}" fill-rule="evenodd" d="{poly_path(FRAME_OUTER)}{holes}"/>'
    if frame_right != frame:
        out += f'<path fill="{frame_right}" d="{poly_path(FRAME_BRONZE)}"/>'
    for shape in FRAME_ACCENTS:
        out += f'<path fill="{frame}" d="{poly_path(shape)}"/>'
    out += f'<path fill="{accent}" d="{"".join(poly_path(s) for s in ACCENTS)}"/>'
    return out


def small_mark_group(left, right, play, window=None):
    """The simplified mark, drawn for sizes of 64 px and below."""
    inner = poly_path(SMALL_INNER)
    out = ""
    if window:
        out += f'<path fill="{window}" d="{inner}"/>'
    out += f'<path fill="{left}" fill-rule="evenodd" d="{poly_path(SMALL_OUTER)}{inner}"/>'
    out += f'<path fill="{right}" d="{poly_path(SMALL_RIGHT)}"/>'
    out += f'<path fill="{play}" d="{poly_path(SMALL_PLAY)}"/>'
    return out


def word_group(color):
    return f'<path fill="{color}" fill-rule="evenodd" d="{WORD_PATH}"/>'
