"""Writes the licences of the libraries built into BareClip's FFmpeg DLLs.

    python scripts/ffmpeg-licenses.py

BareClip ships a ready-made LGPL build of FFmpeg from BtbN/FFmpeg-Builds, and that build has
dozens of other open-source libraries compiled into it, each under its own licence. This finds
them the way BtbN's build does: it takes the switches the DLLs report they were built with, finds
the build script providing each, and follows each script's dependencies, keeping only those its
own test turns on for 64-bit Windows, LGPL shared, FFmpeg 9.0. It only reads those scripts as
text and never runs them. Then it fetches every library's licence files at the exact version
built, and writes them to assets/licenses/FFMPEG-LIBRARIES.txt.

Run it again whenever scripts/setup-ffmpeg.ps1 points at a different build. It needs git and an
internet connection, and keeps what it downloads in target/ffmpeg-licenses.
"""

import fnmatch
import json
import pathlib
import re
import subprocess
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "assets" / "licenses" / "FFMPEG-LIBRARIES.txt"
WORK = ROOT / "target" / "ffmpeg-licenses"
FFMPEG = ROOT / "vendor" / "ffmpeg" / "bin" / "ffmpeg.exe"
BUILDS = "https://github.com/BtbN/FFmpeg-Builds.git"

# What BareClip's download is, as BtbN's scripts see it.
SETTINGS = {"TARGET": "win64", "VARIANT": "lgpl-shared", "ADDINS_STR": "9.0"}
FFMPEG_VERSION = 900

# Build steps and parts of Windows, rather than libraries.
NOT_LIBRARIES = {"base", "finalize", "enable", "schannel"}
# gnulib's pieces are compiled into libiconv and come under libiconv's licence.
LEFT_OUT = {"libiconv2": "gnulib, whose pieces built into libiconv come under libiconv's licence"}
LICENCE_FILE = re.compile(
    r"^(licen[cs]e|copying|copyright|notice|unlicense|patents?)([-_.].*)?$", re.I
)
# Libraries that keep their licence at the top of a source file rather than in a file of its own.
LICENCE_IN_SOURCE = {"ffnvcodec": "include/ffnvcodec/nvEncodeAPI.h"}


def run(*args, folder=None, timeout=600):
    return subprocess.run(
        ["git", *(["-C", str(folder)] if folder else []), *args],
        capture_output=True,
        timeout=timeout,
    )


def builds_tag():
    text = (ROOT / "scripts" / "setup-ffmpeg.ps1").read_text(encoding="utf-8")
    return re.search(r"\$Tag = '([^']+)'", text).group(1)


def build_scripts(tag):
    folder = WORK / "FFmpeg-Builds"
    if not (folder / ".git").exists():
        folder.mkdir(parents=True, exist_ok=True)
        run("init", "-q", folder=folder)
    fetched = run("fetch", "-q", "--depth", "1", BUILDS, f"refs/tags/{tag}", folder=folder)
    if fetched.returncode != 0:
        raise SystemExit(fetched.stderr.decode(errors="replace"))
    run("checkout", "-q", "--force", "FETCH_HEAD", folder=folder)
    commit = run("rev-parse", "HEAD", folder=folder).stdout.decode().strip()
    return folder / "scripts.d", commit


def function_body(text, function):
    match = re.search(function + r"\(\)\s*\{(.*?)\n\}", text, re.S)
    return match.group(1) if match else ""


def holds(test):
    """One of the tests BtbN's scripts use, for these settings."""
    test = test.strip()
    parts = re.split(r"\s+&&\s+(?=\[\[|\(\()", test)
    if len(parts) > 1:
        return all(holds(part) for part in parts)
    match = re.fullmatch(
        r"\[\[\s*\$(\w+)\s*(==|!=)\s*(\S+)\s*(&&\s*\$(\w+)\s*(==|!=)\s*(\S+)\s*)?\]\]", test
    )
    if match:
        def compare(variable, operator, pattern):
            same = fnmatch.fnmatchcase(SETTINGS[variable], pattern)
            return same if operator == "==" else not same

        result = compare(*match.group(1, 2, 3))
        if match.group(4):
            result = result and compare(*match.group(5, 6, 7))
        return result
    match = re.fullmatch(r"\(\(\s*\$\(ffbuild_ffver\)\s*(>=|<=|>|<|==)\s*(\d+)\s*\)\)", test)
    if match:
        number = int(match.group(2))
        return {
            ">=": FFMPEG_VERSION >= number,
            "<=": FFMPEG_VERSION <= number,
            ">": FFMPEG_VERSION > number,
            "<": FFMPEG_VERSION < number,
            "==": FFMPEG_VERSION == number,
        }[match.group(1)]
    raise SystemExit(f"A build script has a test this doesn't understand: {test}")


def switched_on(text):
    for line in function_body(text, "ffbuild_enabled").splitlines():
        line = line.strip()
        # rustdedup checks two other scripts are on, and both are for these settings.
        if not line or line.startswith("#") or line.startswith("( source"):
            continue
        match = re.fullmatch(r"return (-?\d+)", line)
        if match:
            return match.group(1) == "0"
        match = re.fullmatch(r"(.*?)\s+(&&|\|\|)\s+return (-?\d+)", line)
        if match:
            if (match.group(2) == "&&") == holds(match.group(1)):
                return match.group(3) == "0"
            continue
        return holds(line)
    return True


def name_of(path):
    return re.sub(r"^\d\d-", "", path.name).removesuffix(".sh")


def sources_of(text):
    return dict(re.findall(r'^(SCRIPT_[A-Z0-9_]+)="?([^"\n]*)"?', text, re.M))


def libraries(scripts):
    reported = subprocess.run([str(FFMPEG), "-hide_banner", "-buildconf"], capture_output=True, text=True)
    switches = set(re.findall(r"--enable-[a-z0-9-]+", reported.stdout + reported.stderr))
    entries = {}
    for path in sorted(scripts.iterdir()):
        stages = sorted(path.glob("??-*.sh")) if path.is_dir() else [path] if path.suffix == ".sh" else []
        texts = [stage.read_text(encoding="utf-8", errors="replace") for stage in stages]
        if not stages:
            continue
        entries[name_of(path)] = {
            "stages": [(name_of(stage), sources_of(text)) for stage, text in zip(stages, texts) if switched_on(text)],
            "needs": {need for text in texts for need in re.findall(r"echo (\S+)", function_body(text, "ffbuild_depends"))},
            "gives": {flag for text in texts for flag in re.findall(r"--enable-[a-z0-9-]+", function_body(text, "ffbuild_configure"))},
        }
    waiting = [name for name, entry in entries.items() if entry["gives"] & switches]
    included = set()
    while waiting:
        name = waiting.pop()
        if name in included or not entries.get(name, {}).get("stages"):
            continue
        included.add(name)
        waiting.extend(entries[name]["needs"])
    found = {}
    for name in sorted(included):
        for stage, sources in entries[name]["stages"]:
            if stage not in NOT_LIBRARIES:
                found[stage] = sources
    return found


def fetch_texts(key, repo, ref):
    """The licence files of a library at a version, or of its source file for the few that keep it
    there. Only the file list and those files are downloaded."""
    if repo.startswith("https://svn."):
        texts = {}
        for file in ("COPYING", "LICENSE"):
            try:
                with urllib.request.urlopen(f"{repo}/{file}", timeout=60) as response:
                    texts[file] = response.read().decode("utf-8", errors="replace")
            except OSError:
                pass
        return texts
    folder = WORK / "sources" / re.sub(r"[^\w.-]", "_", f"{key}-{ref}")[:80]
    if not (folder / ".git").exists():
        folder.mkdir(parents=True, exist_ok=True)
        run("init", "-q", folder=folder)
    # Some hosts, GNU's among them, turn requests away now and then, so each gets a few tries.
    for _ in range(4):
        if run("fetch", "-q", "--depth", "1", "--filter=blob:none", repo, ref, folder=folder).returncode == 0:
            break
    else:
        raise SystemExit(f"Couldn't fetch {key} from {repo} at {ref}")
    files = run("ls-tree", "--name-only", "FETCH_HEAD", folder=folder).stdout.decode().split("\n")
    texts = {}
    for file in [file for file in files if LICENCE_FILE.match(file)]:
        shown = run("cat-file", "-p", f"FETCH_HEAD:{file}", folder=folder)
        if shown.returncode == 0:
            texts[file] = shown.stdout.decode("utf-8", errors="replace")
    base = re.sub(r"\d$", "", key)
    if not texts and base in LICENCE_IN_SOURCE:
        shown = run("cat-file", "-p", f"FETCH_HEAD:{LICENCE_IN_SOURCE[base]}", folder=folder)
        comment = re.match(r"/\*(.*?)\*/", shown.stdout.decode("utf-8", errors="replace"), re.S)
        if comment:
            lines = [re.sub(r"^\s*\* ?", "", line) for line in comment.group(1).splitlines()]
            texts[LICENCE_IN_SOURCE[base]] = "\n".join(lines).strip("\n") + "\n"
    return texts


def tidy(text):
    return text.replace("\r\n", "\n").strip("\n") + "\n"


def main():
    tag = builds_tag()
    scripts, commit = build_scripts(tag)
    found = libraries(scripts)
    texts, listing, missing = {}, [], []
    for name, sources in found.items():
        for suffix in ("", "2", "3"):
            repo = sources.get(f"SCRIPT_REPO{suffix}")
            if not repo:
                continue
            key = f"{name}{suffix}"
            if key in LEFT_OUT:
                continue
            ref = (sources.get(f"SCRIPT_COMMIT{suffix}") or sources.get(f"SCRIPT_BRANCH{suffix}")
                   or sources.get("SCRIPT_REV") or "HEAD")
            library_texts = [tidy(text) for _, text in sorted(fetch_texts(key, repo, ref).items())]
            if not library_texts:
                missing.append(f"{key} ({repo})")
            numbers = []
            for text in library_texts:
                texts.setdefault(text, []).append(name)
                numbers.append(list(texts).index(text) + 1)
            listing.append(
                f"{name}\n    Source: {repo}\n    Version: {ref}\n"
                f"    Text: {', '.join(f'[{number}]' for number in numbers)}"
            )
            print(f"{key}: {len(library_texts)} texts", flush=True)
    if missing:
        raise SystemExit("No licence text found for:\n  " + "\n  ".join(missing))

    parts = [
        "Libraries built into FFmpeg",
        "===========================",
        "",
        "BareClip's FFmpeg DLLs are a ready-made LGPL build from BtbN/FFmpeg-Builds, and these",
        "open-source libraries are compiled into them. Each is listed with where its source is and",
        "the exact version built, then the licence texts follow, each written once.",
        "",
        f"Build scripts: {BUILDS.removesuffix('.git')}/tree/{tag}",
        f"               commit {commit}",
        "Left out: " + "; ".join(LEFT_OUT.values()) + ".",
        "",
        "This file is made by scripts/ffmpeg-licenses.py.",
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
        parts += ["", "", f"[{number}] {', '.join(dict.fromkeys(users))}", "", text.rstrip("\n")]
    OUTPUT.write_text("\n".join(parts) + "\n", encoding="utf-8", newline="\n")
    print(f"Wrote {OUTPUT.relative_to(ROOT)}: {len(listing)} libraries, {len(texts)} texts")


if __name__ == "__main__":
    main()
