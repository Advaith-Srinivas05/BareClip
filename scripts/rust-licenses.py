"""Writes the licence of every Rust library built into BareClip's Windows app.

    python scripts/rust-licenses.py           writes assets/licenses/RUST-LIBRARIES.txt
    python scripts/rust-licenses.py --check   fails if that file is out of date

It asks Cargo which libraries the app is built from, leaving out ones only used for tests or
build scripts, and copies the licence files each library ships with. A library with none gets the
standard text of its licence. Texts that are word for word the same are written once, with the
libraries that use them listed above.
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "assets" / "licenses" / "RUST-LIBRARIES.txt"
APP = "bareclip-app"
TARGET = "x86_64-pc-windows-msvc"

# Files a library keeps its licence in, at its top level.
LICENCE_NAME = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)([-_.].*)?$", re.I)

MIT_TEXT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


WTFPL_TEXT = """DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
Version 2, December 2004

Copyright (C) 2004 Sam Hocevar <sam@hocevar.net>

Everyone is permitted to copy and distribute verbatim or modified
copies of this license document, and changing it is allowed as long
as the name is changed.

DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

0. You just DO WHAT THE FUCK YOU WANT TO.
"""


def metadata():
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
        cwd=ROOT,
        capture_output=True,
        check=True,
    )
    return json.loads(result.stdout)


def shipped_packages(data):
    """The libraries the app is built from, following only normal dependencies."""
    packages = {package["id"]: package for package in data["packages"]}
    nodes = {node["id"]: node for node in data["resolve"]["nodes"]}
    workspace = set(data["workspace_members"])
    root = next(id for id in workspace if packages[id]["name"] == APP)
    seen, waiting = set(), [root]
    while waiting:
        id = waiting.pop()
        if id in seen:
            continue
        seen.add(id)
        for dependency in nodes[id]["deps"]:
            if any(kind["kind"] is None for kind in dependency["dep_kinds"]):
                waiting.append(dependency["pkg"])
    return sorted(
        (packages[id] for id in seen - workspace),
        key=lambda package: (package["name"].lower(), package["version"]),
    )


def read_text(path):
    text = path.read_bytes().decode("utf-8", errors="replace").replace("\r\n", "\n")
    return text.strip("\n") + "\n"


def licence_files(package):
    folder = pathlib.Path(package["manifest_path"]).parent
    found = sorted(
        path for path in folder.iterdir() if path.is_file() and LICENCE_NAME.match(path.name)
    )
    if package.get("license_file"):
        named = folder / package["license_file"]
        if named.is_file() and named not in found:
            found.append(named)
    return found


def font_licence_files(package):
    """Fonts carried inside a library, such as egui's, each come with a licence of its own, in a
    text file beside them."""
    expression = package.get("license") or ""
    if "OFL" not in expression and "Ubuntu-font" not in expression:
        return []
    folder = pathlib.Path(package["manifest_path"]).parent
    return sorted(path for path in folder.rglob("*.txt") if path.parent != folder)


def standard_texts(package, apache_text):
    """Stands in for a library that ships no licence file, from what its manifest says. The
    Apache text is the one other libraries here ship, word for word."""
    expression = package.get("license") or ""
    authors = ", ".join(package.get("authors") or []) or f"The {package['name']} authors"
    if "MIT" in expression:
        return [f"MIT License\n\nCopyright (c) {authors}\n\n{MIT_TEXT}"]
    if "Apache-2.0" in expression and apache_text:
        return [apache_text]
    if "WTFPL" in expression:
        return [WTFPL_TEXT]
    return []


def apache_text_of(packages):
    """The Apache 2.0 text most of these libraries ship."""
    counts = {}
    for package in packages:
        for path in licence_files(package):
            if "apache" in path.name.lower():
                text = read_text(path)
                if "Apache License" in text[:300]:
                    counts[text] = counts.get(text, 0) + 1
    return max(counts, key=counts.get) if counts else None


def build():
    packages = shipped_packages(metadata())
    apache_text = apache_text_of(packages)
    texts = {}  # text -> the libraries using it, in order
    listing = []
    missing = []
    for package in packages:
        label = f"{package['name']} {package['version']}"
        own = [read_text(path) for path in licence_files(package)]
        package_texts = (own or standard_texts(package, apache_text)) + [
            read_text(path) for path in font_licence_files(package)
        ]
        if not package_texts:
            missing.append(f"{label} ({package.get('license')})")
        numbers = []
        for text in package_texts:
            texts.setdefault(text, []).append(label)
            numbers.append(list(texts).index(text) + 1)
        where = package.get("repository") or package.get("homepage") or "crates.io"
        listing.append(
            f"{label}\n    Licence: {package.get('license')}\n    Source: {where}\n"
            f"    Text: {', '.join(f'[{number}]' for number in numbers)}"
        )
    if missing:
        raise SystemExit("No licence text found for:\n  " + "\n  ".join(missing))

    parts = [
        "Rust libraries in BareClip",
        "==========================",
        "",
        f"BareClip's Windows app is built from these {len(packages)} open-source Rust libraries.",
        "Each is listed with its licence and where its source is, then the licence texts",
        "follow, each written once with the libraries it belongs to.",
        "",
        "This file is made by scripts/rust-licenses.py from Cargo.lock.",
        "",
        "",
        "Libraries",
        "---------",
        "",
        "\n\n".join(listing),
        "",
        "",
        "Licence texts",
        "-------------",
    ]
    for number, (text, users) in enumerate(texts.items(), start=1):
        parts += ["", "", f"[{number}] {', '.join(users)}", "", text.rstrip("\n")]
    return "\n".join(parts) + "\n"


def main():
    made = build()
    if "--check" in sys.argv:
        current = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        if current.replace("\r\n", "\n") != made:
            raise SystemExit(
                f"{OUTPUT.relative_to(ROOT)} is out of date. Run python scripts/rust-licenses.py"
            )
        print("The Rust licence list is up to date.")
        return
    OUTPUT.write_text(made, encoding="utf-8", newline="\n")
    print(f"Wrote {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
