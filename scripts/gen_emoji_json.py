#!/usr/bin/env python3
"""Generates core-rs/src/emoji.json (emoji picker data) from the Unicode emoji-test.txt: [[glyph, name, group], ...].
Fully-qualified emoji without skin-tone variants, like scripts/gen_emoji.py does for the Widgets UI."""
import json, re, sys

src = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/unicode/emoji/emoji-test.txt"
groups, items, group = [], [], -1
for line in open(src, encoding="utf-8"):
    line = line.rstrip("\n")
    if line.startswith("# group:"):
        groups.append(line.split(":", 1)[1].strip())
        group = len(groups) - 1
        continue
    if not line or line.startswith("#"):
        continue
    m = re.match(r"^([0-9A-F ]+)\s*;\s*fully-qualified\s*#\s*(\S+)\s+E[0-9.]+\s+(.*)$", line)
    if not m or group < 0 or groups[group] == "Component":
        continue
    glyph, name = m.group(2), m.group(3)
    if "skin tone" in name or ("hair" in name and "person:" in name):
        continue
    items.append([glyph, name, groups[group]])
with open("core-rs/src/emoji.json", "w", encoding="utf-8") as f:
    json.dump(items, f, ensure_ascii=False, separators=(",", ":"))
print(len(items), "emoji")
