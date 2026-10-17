"""Writes THIRD-PARTY-LICENSES.txt for LANcast.exe to stdout.

Lists every crate compiled into the binary (normal dependencies only;
build-time tools such as bindgen are not shipped) with its license
expression and the license/notice files it ships.

Usage (from the project root): python tools/third_party_licenses.py [OUTPUT_FILE]
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT_PACKAGE = "lancast"
TARGET = "x86_64-pc-windows-msvc"
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "copyright")

metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", TARGET],
        check=True,
        capture_output=True,
        encoding="utf-8",
    ).stdout
)
packages = {p["id"]: p for p in metadata["packages"]}
nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
root = next(p["id"] for p in metadata["packages"] if p["name"] == ROOT_PACKAGE)

# Walk normal (non-build, non-dev) dependencies from the root package.
shipped, todo = set(), [root]
while todo:
    node = nodes[todo.pop()]
    for dep in node["deps"]:
        if any(kind["kind"] is None for kind in dep["dep_kinds"]) and dep["pkg"] not in shipped:
            shipped.add(dep["pkg"])
            todo.append(dep["pkg"])

# Write to the given file (UTF-8) or to stdout.
if len(sys.argv) > 1:
    out = open(sys.argv[1], "w", encoding="utf-8", newline="\r\n")
else:
    out = sys.stdout
    out.reconfigure(encoding="utf-8")
out.write("Third-party software in LANcast.exe\n")
out.write("===================================\n\n")
out.write(
    "LANcast.exe includes the following open-source Rust crates. Their license\n"
    "terms and notices follow below. (The NDI(R) runtime is covered separately:\n"
    "see EULA.txt and Processing.NDI.Lib.Licenses.txt.)\n\n"
)
crates = sorted((packages[i] for i in shipped), key=lambda p: (p["name"], p["version"]))
for p in crates:
    out.write(f"- {p['name']} {p['version']}: {p.get('license') or 'see license file'}\n")

seen_texts = {}


def apache_reference():
    """Where the Apache License 2.0 text was printed first."""
    for text, where in seen_texts.items():
        if "Apache License" in text and "Version 2.0, January 2004" in text:
            return where
    return "see https://www.apache.org/licenses/LICENSE-2.0"


# Print crates that ship license files first, so the Apache text is already
# included when a crate without its own file refers to it.
def has_license_file(p):
    directory = Path(p["manifest_path"]).parent
    return any(f.name.lower().startswith(LICENSE_PREFIXES) for f in directory.iterdir() if f.is_file())


crates.sort(key=lambda p: (not has_license_file(p), p["name"], p["version"]))
for p in crates:
    directory = Path(p["manifest_path"]).parent
    files = sorted(
        f for f in directory.iterdir()
        if f.is_file() and f.name.lower().startswith(LICENSE_PREFIXES)
    )
    if p.get("license_file"):
        extra = directory / p["license_file"]
        if extra.is_file() and extra not in files:
            files.append(extra)
    # Bundled assets with their own licenses (e.g. egui's default fonts).
    for sub in ("fonts", "licenses", "LICENSES"):
        if (directory / sub).is_dir():
            files += sorted((directory / sub).glob("*.txt"))
    out.write("\n" + "=" * 78 + "\n")
    out.write(f"{p['name']} {p['version']}\n")
    out.write(f"License: {p.get('license') or 'see below'}\n")
    if p.get("repository"):
        out.write(f"Repository: {p['repository']}\n")
    if not files:
        if "Apache-2.0" in (p.get("license") or ""):
            out.write(
                "(The crate ships no license file. LANcast uses it under the Apache License 2.0,\n"
                f"whose full text is included above: {apache_reference()}.)\n"
            )
        else:
            out.write("(The crate ships no license file; see the license expression above.)\n")
    for f in files:
        text = f.read_text(encoding="utf-8", errors="replace").strip()
        # Identical license texts (e.g. the Apache License) are printed once.
        if text in seen_texts:
            out.write(f"\n--- {f.name}: identical to {seen_texts[text]} ---\n")
            continue
        seen_texts[text] = f"{f.name} of {p['name']} {p['version']}"
        out.write(f"\n--- {f.name} ---\n{text}\n")
