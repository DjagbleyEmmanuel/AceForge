#!/usr/bin/env python3
"""Fail if a <Select> sits under a wrapper that already renders its label.

`Select` draws its own label from the `label` prop. Several call sites kept a
`<label className="label">` in the surrounding markup from before the native
<select> was replaced, so the same caption printed twice. That happened four
more times after the first two were fixed one at a time, so it is checked
rather than remembered.

    python3 scripts/check-duplicate-labels.py

Exits non-zero when a duplicate is found.
"""
import re, glob, sys
bad = 0
for f in sorted(glob.glob("src/components/*.tsx")):
    if f.endswith("Select.tsx"): continue
    lines = open(f).read().split("\n")
    for i, l in enumerate(lines):
        if "<Select" not in l: continue
        chunk = "\n".join(lines[i:i+8])
        m = re.search(r'label="([^"]+)"', chunk)
        mv = re.search(r'label=\{(\w+)\}', chunk)
        name = m.group(1) if m else (mv.group(1) if mv else None)
        if not name: continue
        back = "\n".join(lines[max(0, i-7):i])
        if re.search(r'<label className="label"[^>]*>\s*' + re.escape(name) + r'\s*</label>', back, re.S):
            print(f"  DUPLICATE  {f}:{i+1}  '{name}'"); bad += 1
print("duplicates:", bad)
sys.exit(1 if bad else 0)
