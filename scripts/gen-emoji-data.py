#!/usr/bin/env python3
"""Generate crates/stickpile-server/assets/emoji.json, the data of the web
UI's emoji picker (assets/emoji.js). Standard library only.

    python3 scripts/gen-emoji-data.py            # Unicode data from the flake's nixpkgs
    python3 scripts/gen-emoji-data.py --emoji-test PATH --cldr DIR --cldr-version 48.2

Sources:

* emoji-test.txt (nixpkgs#unicode-emoji): the emojis, their order and
  groups. Only fully-qualified emojis; skin-tone and hair-style variants
  and the Component group are left out.
* CLDR annotations/en.xml and annotationsDerived/en.xml
  (nixpkgs#cldr-annotations): English names (`type="tts"`, else the name in
  emoji-test.txt) and keywords.
* crates/stickpile-search/src/emoji_names.rs: the slang the server's search
  knows ("lol", "sus"), so the picker finds what search finds.

Output: {"notice", "license", "emoji", "cldr", "common": [emoji, ...],
"groups": [[group, tab emoji, [[emoji, name, keywords, terms?], ...]], ...]}.
`terms` are the curated words of emoji_names.rs (left out when there are
none), which the picker ranks above the CLDR `keywords`; both are one string
of lowercase words separated by spaces, and `keywords` leaves out the words
of the name and of `terms`. `common` lists the emojis of emoji_names.rs in
its order (the ones used most on stickers, roughly most used first): the
picker's tie-breaker and its suggestions before anything was used.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "crates", "stickpile-server", "assets", "emoji.json")
SLANG = os.path.join(ROOT, "crates", "stickpile-search", "src", "emoji_names.rs")

SKIN_TONES = set(range(0x1F3FB, 0x1F400))
HAIR = set(range(0x1F9B0, 0x1F9B4))  # red, curly, white hair, bald
VS16 = "️"
ZWJ = "‍"

# The emoji on each category tab (the first of a group is not always telling).
TAB = {
    "Smileys & Emotion": "😀",
    "People & Body": "👋",
    "Animals & Nature": "🐻",
    "Food & Drink": "🍔",
    "Travel & Places": "🚗",
    "Activities": "⚽",
    "Objects": "💡",
    "Symbols": "💕",
    "Flags": "🏁",
}


def nix_build(attr):
    out = subprocess.run(
        ["nix", "build", "--no-link", "--print-out-paths", "--inputs-from", ROOT, "nixpkgs#" + attr],
        check=True,
        capture_output=True,
        text=True,
    )
    return out.stdout.strip().splitlines()[-1]


def parse_emoji_test(path):
    """[(group, [(emoji, name), ...]), ...] and the emoji version."""
    groups = []
    version = None
    group = None
    line_re = re.compile(r"^([0-9A-F ]+?)\s*;\s*([a-z-]+)\s*#\s*\S+\s+E[0-9.]+\s+(.+)$")
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("# Version:"):
                version = line.split(":", 1)[1].strip()
            elif line.startswith("# group:"):
                group = line.split(":", 1)[1].strip()
                if group != "Component":
                    groups.append((group, []))
            elif line and not line.startswith("#"):
                m = line_re.match(line)
                if not m:
                    raise SystemExit("unexpected line in emoji-test.txt: " + line)
                cps = [int(c, 16) for c in m.group(1).split()]
                if m.group(2) != "fully-qualified" or group == "Component":
                    continue
                if any(c in SKIN_TONES or c in HAIR for c in cps):
                    continue
                groups[-1][1].append(("".join(map(chr, cps)), m.group(3)))
    if not version:
        raise SystemExit("no version in emoji-test.txt")
    return groups, version


def parse_annotations(*paths):
    """{emoji without VS16: (keywords, tts name)}; later files fill gaps."""
    out = {}
    copyright_years = None
    for path in paths:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        m = re.search(r"Copyright © (\d{4}(?:-\d{4})?) Unicode, Inc\.", text)
        if m and not copyright_years:
            copyright_years = m.group(1)
        for a in ET.fromstring(text.encode("utf-8")).iter("annotation"):
            cp = a.get("cp")
            kw, tts = out.get(cp, (None, None))
            if a.get("type") == "tts":
                tts = tts or a.text.strip()
            else:
                kw = kw or [k.strip() for k in a.text.split("|") if k.strip()]
            out[cp] = (kw, tts)
    return out, copyright_years


def parse_slang(path):
    """[(emoji without VS16, words), ...] in the file's order."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    return re.findall(r'\("([^"]+)", "([^"]+)"\)', text)


def norm(s):
    return s.replace("’", "'").replace("“", '"').replace("”", '"')


def words(s):
    return [w for w in re.split(r"[\s:,()\"]+", norm(s).lower()) if w]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--emoji-test", help="emoji-test.txt (default: from nixpkgs#unicode-emoji)")
    p.add_argument("--cldr", help="CLDR common/ directory (default: from nixpkgs#cldr-annotations)")
    p.add_argument("--cldr-version", help="CLDR version (default: from the nix store path)")
    p.add_argument("--out", default=OUT)
    args = p.parse_args()

    emoji_test = args.emoji_test
    if not emoji_test:
        emoji_test = os.path.join(nix_build("unicode-emoji"), "share/unicode/emoji/emoji-test.txt")
    cldr = args.cldr
    if not cldr:
        cldr = os.path.join(nix_build("cldr-annotations"), "share/unicode/cldr/common")
    cldr_version = args.cldr_version
    if not cldr_version:
        m = re.search(r"cldr-annotations-([0-9.]+)", os.path.realpath(cldr))
        if not m:
            raise SystemExit("pass --cldr-version")
        cldr_version = m.group(1)

    groups, emoji_version = parse_emoji_test(emoji_test)
    notes, years = parse_annotations(
        os.path.join(cldr, "annotations", "en.xml"), os.path.join(cldr, "annotationsDerived", "en.xml")
    )
    slang_list = parse_slang(SLANG)
    slang = dict(slang_list)

    out_groups = []
    total = 0
    qualified = {}  # emoji without VS16 -> as in emoji-test.txt
    for group, entries in groups:
        rows = []
        for emoji, name in entries:
            key = emoji.replace(VS16, "")
            qualified.setdefault(key, emoji)
            kw, tts = notes.get(key, (None, None))
            name = norm(tts or name)
            extra = slang.get(key)
            if extra is None and ZWJ in key:
                extra = slang.get(key[0])  # like the search: 🤷‍♂ -> 🤷
            # The curated terms stay whole (also words of the name: they
            # rank higher); the keywords leave out what is said already.
            terms = []
            for w in words(extra or ""):
                if w not in terms:
                    terms.append(w)
            have = set(words(name)) | set(terms)
            keywords = []
            for w in words(" ".join(kw or [])):
                if w not in have:
                    have.add(w)
                    keywords.append(w)
            row = [emoji, name, " ".join(keywords)]
            if terms:
                row.append(" ".join(terms))
            rows.append(row)
        total += len(rows)
        out_groups.append([group, TAB.get(group, entries[0][0]), rows])

    common = []
    for key, _ in slang_list:
        if key not in qualified:
            raise SystemExit("emoji_names.rs: {} is not in emoji-test.txt".format(key))
        if qualified[key] not in common:
            common.append(qualified[key])

    data = {
        "notice": (
            "Emoji list, names and keywords from Unicode emoji-test.txt {e} and Unicode CLDR {c} "
            "English annotations (annotations/en.xml, annotationsDerived/en.xml), with search terms "
            "from stickpile-search. Copyright © {y} Unicode, Inc. Used under the Unicode License v3 "
            "(https://www.unicode.org/license.txt). Generated by scripts/gen-emoji-data.py."
        ).format(e=emoji_version, c=cldr_version, y=years or "1991-2026"),
        "license": "Unicode-3.0",
        "emoji": emoji_version,
        "cldr": cldr_version,
        "common": common,
        "groups": out_groups,
    }
    text = json.dumps(data, ensure_ascii=False, separators=(",", ":"))
    # One emoji per line keeps diffs of a regeneration readable.
    text = text.replace(',["', ',\n["').replace('"groups":[[', '"groups":[\n[').replace('"common":[', '\n"common":[')
    with open(args.out, "w", encoding="utf-8") as f:
        f.write(text + "\n")
    print("wrote {} ({} emojis, {} bytes)".format(args.out, total, len(text.encode("utf-8")) + 1), file=sys.stderr)


if __name__ == "__main__":
    main()
