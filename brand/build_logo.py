#!/usr/bin/env python3
"""Build the RepoTracer logo system: mark, wordmark (outlined), lockups, favicon."""
import pathlib
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

S = pathlib.Path("/private/tmp/claude-502/-Users-mac-Documents-projects-tools-tool/740785db-d7fc-4c5b-9864-97c7ff470c03/scratchpad")
OUT = pathlib.Path("brand/logo"); OUT.mkdir(parents=True, exist_ok=True)
import re as _re
def _round(s, nd=1):
    return _re.sub(r"-?\d+\.\d+", lambda m: f"{float(m.group()):.{nd}f}".rstrip("0").rstrip("."), s)


INK   = "#14181D"
BONE  = "#F6F5F1"
TEAL  = "#0E9488"

# ---------------------------------------------------------------- the mark
XS = [20, 35, 50, 65, 80]
ROUTE = [(20, 80, 3.0), (35, 65, 3.67), (50, 50, 4.33), (65, 35, 5.0)]
HIT = (80, 35, 7.6)
FIELD = [(x, y) for y in XS for x in XS
         if (x, y) not in {(a, b) for a, b, _ in ROUTE} and (x, y) != HIT[:2]]

def mark(ink=INK, hit=TEAL, field_op=0.20):
    dots = "".join(f'<circle cx="{x}" cy="{y}" r="2.4"/>' for x, y in FIELD)
    trail = "".join(f'<circle cx="{x}" cy="{y}" r="{r}"/>' for x, y, r in ROUTE)
    return f'''<g class="rt-mark">
  <g class="rt-field" fill="{ink}" opacity="{field_op}">{dots}</g>
  <g class="rt-trail" fill="{ink}">{trail}</g>
  <circle class="rt-hit" cx="{HIT[0]}" cy="{HIT[1]}" r="{HIT[2]}" fill="{hit}"/>
</g>'''

def svg(w, h, body, vb=None, label="RepoTracer"):
    vb = vb or f"0 0 {w} {h}"
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" '
            f'viewBox="{vb}" fill="none" role="img" aria-label="{label}">\n{body}\n</svg>\n')

# ------------------------------------------------------------ the wordmark
def outline(text, ttf, size, tracking=0.0):
    """Return (svg path data, advance width) with the text outlined at `size` em px."""
    font = TTFont(ttf)
    upem = font["head"].unitsPerEm
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    scale = size / upem
    pen_out, x = [], 0.0
    for ch in text:
        gname = cmap[ord(ch)]
        spen = SVGPathPen(gs)
        tpen = TransformPen(spen, (scale, 0, 0, -scale, x, 0))
        gs[gname].draw(tpen)
        d = spen.getCommands()
        if d:
            pen_out.append(d)
        x += gs[gname].width * scale + tracking * size
    return " ".join(pen_out), x - tracking * size

WORD = "RepoTracer"
SIZE = 100.0
TRACK = -0.022
d, adv = outline(WORD, S / "PlexSans600.ttf", SIZE, TRACK)

# Plex Sans cap height = 698/1000 em; so caps sit from y=-69.8 to 0 at size 100.
CAP = 69.8
DESC = 21.2   # descender of 'p' ≈ -212/1000 em

# --------------------------------------------------------------- write out
(OUT / "mark.svg").write_text(_round(svg(100, 100, mark())))
(OUT / "mark-mono.svg").write_text(_round(svg(100, 100, mark(ink="currentColor", hit="currentColor"))))
(OUT / "mark-inverse.svg").write_text(_round(svg(100, 100, mark(ink=BONE, hit=TEAL))))

# favicon: 3x3 diagonal so it survives 16px
FAV_ROUTE = [(26, 74, 5.0), (50, 50, 6.4)]
FAV_FIELD = [(x, y) for y in (26, 50, 74) for x in (26, 50, 74)
             if (x, y) not in {(a, b) for a, b, _ in FAV_ROUTE} and (x, y) != (74, 26)]
fav_dots = "".join(f'<circle cx="{x}" cy="{y}" r="4"/>' for x, y in FAV_FIELD)
fav_trail = "".join(f'<circle cx="{x}" cy="{y}" r="{r}"/>' for x, y, r in FAV_ROUTE)
fav = f'''<rect width="100" height="100" rx="22" fill="{INK}"/>
  <g fill="{BONE}" opacity="0.26">{fav_dots}</g>
  <g fill="{BONE}">{fav_trail}</g>
  <circle cx="74" cy="26" r="11" fill="{TEAL}"/>'''
(OUT / "favicon.svg").write_text(_round(svg(100, 100, fav)))

def lockup(name, ink, hit, wm_fill, bg=None):
    """Mark + outlined wordmark on a shared baseline."""
    cap = 62.0                       # wordmark cap height in lockup units
    fs = cap / (CAP / SIZE)          # font size that yields that cap height
    dd, aa = outline(WORD, S / "PlexSans600.ttf", fs, TRACK)
    m = 92.0                         # mark box side
    gap = 26.0
    pad = 8.0
    baseline = pad + m * 0.5 + cap * 0.5      # optically centre caps on the mark
    w = pad + m + gap + aa + pad
    h = pad + m + pad
    body = (f'  <g transform="translate({pad},{pad})">\n'
            f'    <g transform="scale({m/100})">{mark(ink, hit)}</g>\n  </g>\n'
            f'  <path class="rt-word" transform="translate({pad+m+gap},{baseline})" d="{dd}" fill="{wm_fill}"/>')
    if bg:
        body = f'  <rect width="{w:.1f}" height="{h:.1f}" fill="{bg}"/>\n' + body
    (OUT / name).write_text(_round(svg(round(w), round(h), body, vb=f"0 0 {w:.1f} {h:.1f}")))
    return w, h

lockup("lockup.svg",         INK,          TEAL, INK)
lockup("lockup-mono.svg",    "currentColor","currentColor","currentColor")
lockup("lockup-inverse.svg", BONE,         TEAL, BONE, bg=None)

# stacked lockup
cap = 40.0
fs = cap / (CAP / SIZE)
dd, aa = outline(WORD, S / "PlexSans600.ttf", fs, TRACK)
m = 108.0
w = max(m, aa)
desc = 0.212 * fs
h = m + 26 + cap + desc
body = (f'  <g transform="translate({(w-m)/2:.1f},0) scale({m/100})">{mark()}</g>\n'
        f'  <path transform="translate({(w-aa)/2:.1f},{m+22+cap:.1f})" d="{dd}" fill="{INK}"/>')
(OUT / "lockup-stacked.svg").write_text(_round(svg(round(w), round(h), body, vb=f"0 0 {w:.1f} {h:.1f}")))

for p in sorted(OUT.glob("*.svg")):
    print(f"{p}  {p.stat().st_size}B")
