"""A fake $EDITOR for `diffnote edit`: applies the lines of the file named by
$DN_SCRIPT to the buffer it is given.

Each line is `AFTER<TAB>TEXT`: `> TEXT` is put after the first buffer line equal
to AFTER (`GLOBAL`: on top instead). An AFTER that starts with `@raw:` puts TEXT
in as it is (for a directive such as `>!resolve`).
"""
import os
import sys

buffer = sys.argv[-1]
with open(buffer, encoding="utf-8", newline="") as f:
    lines = f.read().split("\n")
if lines and lines[-1] == "":
    lines.pop()
with open(os.environ["DN_SCRIPT"], encoding="utf-8") as f:
    script = [l.rstrip("\n") for l in f if l.strip()]
for entry in script:
    after, _, text = entry.partition("\t")
    if after == "GLOBAL":
        lines[0:0] = ["> " + text, ""]
        continue
    raw = after.startswith("@raw:")
    if raw:
        after = after[len("@raw:"):]
    # After the last thing already added for this line (so a directive follows
    # its comment).
    for i, line in enumerate(lines):
        if line == after:
            j = i + 1
            while j < len(lines) and lines[j].startswith(">"):
                j += 1
            lines.insert(j, text if raw else "> " + text)
            break
with open(buffer, "w", encoding="utf-8", newline="") as f:
    f.write("\n".join(lines) + "\n")
