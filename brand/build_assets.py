#!/usr/bin/env python3
"""Build RepoTracer README/social graphics. All type is outlined, so the SVGs
render identically on GitHub, on the site, and in any converter."""
import pathlib
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

S = pathlib.Path("/private/tmp/claude-502/-Users-mac-Documents-projects-tools-tool/740785db-d7fc-4c5b-9864-97c7ff470c03/scratchpad")
OUT = pathlib.Path("assets"); OUT.mkdir(exist_ok=True)
import re as _re
def _round(s, nd=1):
    return _re.sub(r"-?\d+\.\d+", lambda m: f"{float(m.group()):.{nd}f}".rstrip("0").rstrip("."), s)


INK, SLATE, BONE = "#14181D", "#1B2027", "#F6F5F1"
PANEL, LINE, MUTED = "#FCFCFA", "#DFDFD8", "#5F6670"
TEAL, TEAL_DEEP = "#0E9488", "#0A6E66"
DIM, LINE_D = "#9AA0A8", "#2B3138"

FONTS = {
    ("sans", 400): "PlexSans400.ttf", ("sans", 500): "PlexSans500.ttf",
    ("sans", 600): "PlexSans600.ttf",
    ("mono", 400): "PlexMono400.ttf", ("mono", 500): "PlexMono500.ttf",
    ("mono", 600): "PlexMono600.ttf",
}
_cache = {}
def _font(fam, w):
    key = (fam, w)
    if key not in _cache:
        f = TTFont(S / FONTS[key])
        _cache[key] = (f, f.getGlyphSet(), f.getBestCmap(), f["head"].unitsPerEm)
    return _cache[key]

def measure(t, fam, w, size, track=0.0):
    _, gs, cmap, upem = _font(fam, w)
    adv = sum(gs[cmap[ord(c)]].width for c in t) * size / upem
    return adv + track * size * max(len(t) - 1, 0)

def T(t, x, y, size, fam="sans", w=400, fill=INK, track=0.0, anchor="start", op=None):
    """Outlined text as a <path>. y is the baseline."""
    _, gs, cmap, upem = _font(fam, w)
    sc = size / upem
    if anchor == "middle":  x -= measure(t, fam, w, size, track) / 2
    elif anchor == "end":   x -= measure(t, fam, w, size, track)
    d, cur = [], x
    for ch in t:
        g = cmap[ord(ch)]
        pen = SVGPathPen(gs)
        gs[g].draw(TransformPen(pen, (sc, 0, 0, -sc, cur, y)))
        if pen.getCommands(): d.append(pen.getCommands())
        cur += gs[g].width * sc + track * size
    o = f' opacity="{op}"' if op else ""
    return f'<path d="{" ".join(d)}" fill="{fill}"{o}/>'

_XS = [20, 35, 50, 65, 80]
_ROUTE = [(20, 80, 3.0), (35, 65, 3.67), (50, 50, 4.33), (65, 35, 5.0)]
_HIT = (80, 35, 7.6)
_FIELD = [(x, y) for y in _XS for x in _XS
          if (x, y) not in {(a, b) for a, b, _ in _ROUTE} and (x, y) != _HIT[:2]]

def mark(x, y, size, ink=INK, hit=TEAL, op="0.20"):
    dots = "".join(f'<circle cx="{a}" cy="{b}" r="2.4"/>' for a, b in _FIELD)
    trail = "".join(f'<circle cx="{a}" cy="{b}" r="{r}"/>' for a, b, r in _ROUTE)
    return (f'<g transform="translate({x},{y}) scale({size/100})">'
            f'<g fill="{ink}" opacity="{op}">{dots}</g>'
            f'<g fill="{ink}">{trail}</g>'
            f'<circle cx="{_HIT[0]}" cy="{_HIT[1]}" r="{_HIT[2]}" fill="{hit}"/></g>')

def svg(w, h, body, label):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" '
            f'viewBox="0 0 {w} {h}" role="img" aria-label="{label}">\n{body}\n</svg>\n')

def write(name, s):
    (OUT / name).write_text(_round(s)); print(f"assets/{name}  {len(s)//1024}KB")

# ------------------------------------------------------------------ buttons
def button(name, text, fill, fg, border=None, sub=None):
    pad, size = 20, 13.5
    w = round(measure(text, "sans", 600, size, 0.005) + pad * 2)
    h = 38
    b = f'<rect x=".5" y=".5" width="{w-1}" height="{h-1}" rx="7" fill="{fill}"' + \
        (f' stroke="{border}"/>' if border else '/>')
    b += T(text, w/2, 24.5, size, "sans", 600, fg, 0.005, "middle")
    write(name, svg(w, h, b, text))

button("button-install.svg",    "Install",    INK,   BONE)
button("button-benchmarks.svg", "Benchmarks", BONE,  INK, LINE)
button("button-website.svg",    "Website",    BONE,  INK, LINE)
button("button-github.svg",     "GitHub",     BONE,  INK, LINE)

# --------------------------------------------------------------- flow chart
W, H = 1200, 452
b = [f'<rect width="{W}" height="{H}" fill="{BONE}"/>']
b.append(T("HOW IT WORKS", 56, 56, 11.5, "mono", 500, MUTED, 0.14))
b.append(T("Same Codex prompt. Less repo searching.", 56, 116, 44, "sans", 600, INK, -0.022))

CW, GAP, CY, CH = 344, 40, 168, 236
steps = [
    ("01", "You prompt Codex", None),
    ("02", "RepoTracer finds the code", None),
    ("03", "Codex edits", None),
]
for i, (num, title, _) in enumerate(steps):
    x = 56 + i * (CW + GAP)
    b.append(f'<rect x="{x}" y="{CY}" width="{CW}" height="{CH}" rx="6" fill="{PANEL}" stroke="{LINE}"/>')
    b.append(T(num, x + 24, CY + 38, 11.5, "mono", 600, TEAL_DEEP, 0.14))
    b.append(T(title, x + 24, CY + 74, 21, "sans", 600, INK, -0.015))
    b.append(f'<path d="M{x+24} {CY+94} H{x+CW-24}" stroke="{LINE}"/>')
    if i:  # connector chevron
        cx = x - GAP / 2
        b.append(f'<path d="M{cx-4} {CY+CH/2-7} l7 7 -7 7" fill="none" stroke="{MUTED}" '
                 f'stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>')

x0 = 56
b.append(f'<rect x="{x0+24}" y="{CY+118}" width="{CW-48}" height="56" rx="5" fill="{INK}"/>')
b.append(T("$", x0 + 42, CY + 148, 12.5, "mono", 500, TEAL))
b.append(T("codex \"fix the refresh token bug\"", x0 + 58, CY + 148, 12.5, "mono", 400, BONE))
b.append(T("no new syntax to learn", x0 + 24, CY + 216, 12.5, "mono", 400, MUTED))

x1 = 56 + CW + GAP
b.append(mark(x1 + 24, CY + 112, 56))
for j, (f, ln) in enumerate([("src/auth/rotate.rs", "88-141"), ("src/auth/session.rs", "12-30")]):
    yy = CY + 128 + j * 30
    b.append(f'<circle cx="{x1+100}" cy="{yy-4}" r="3.4" fill="{TEAL}"/>')
    b.append(T(f, x1 + 112, yy, 12.5, "mono", 500, INK))
    b.append(T(":" + ln, x1 + 112 + measure(f, "mono", 500, 12.5), yy, 12.5, "mono", 400, MUTED))
b.append(T("verified before it is returned", x1 + 24, CY + 216, 12.5, "mono", 400, MUTED))

x2 = 56 + 2 * (CW + GAP)
diff = [("-", "let t = store.get(id)?;", MUTED), ("+", "let t = store.rotate(id)?;", INK),
        ("+", "audit::log(&t);", INK)]
for j, (sign, code, col) in enumerate(diff):
    yy = CY + 132 + j * 26
    b.append(T(sign, x2 + 24, yy, 12.5, "mono", 600, TEAL_DEEP if sign == "+" else MUTED))
    b.append(T(code, x2 + 40, yy, 12.5, "mono", 400, col))
b.append(T("the model spends its tokens here", x2 + 24, CY + 216, 12.5, "mono", 400, MUTED))

b.append(f'<path d="M56 {H-52} H{W-56}" stroke="{LINE}"/>')
b.append(T("RepoTracer searches. Codex edits.", 56, H - 24, 13.5, "sans", 500, MUTED, -0.005))
write("readme-flow.svg", svg(W, H, "\n".join(b), "How RepoTracer works: you prompt Codex, RepoTracer finds the code, Codex edits"))

# ---------------------------------------------------------------- cost card
W, H = 1200, 340
b = [f'<rect width="{W}" height="{H}" fill="{INK}"/>']
b.append(T("REAL-REPO TESTS  ·  NORMAL PROMPTS  ·  SCOUT INCLUDED", 62, 60, 11.5, "mono", 500, DIM, 0.14))
b.append(T("Up to 50% less for the whole task.", 62, 136, 52, "sans", 600, BONE, -0.026))
b.append(T("RepoTracer’s own usage is counted inside that number.", 62, 174, 17, "sans", 400, DIM))
BX, BW, BY = 62, 700, 216
for j, (lab, frac, col, val) in enumerate([("Codex alone", 1.0, "#39414A", "100%"),
                                           ("Codex + RepoTracer", 0.5, TEAL, "50%")]):
    yy = BY + j * 50
    b.append(f'<rect x="{BX}" y="{yy}" width="{BW}" height="30" rx="4" fill="{SLATE}"/>')
    b.append(f'<rect x="{BX}" y="{yy}" width="{BW*frac}" height="30" rx="4" fill="{col}"/>')
    b.append(T(lab, BX + 14, yy + 20, 13, "mono", 500, INK if j else DIM))
    b.append(T(val, BX + BW + 18, yy + 20, 13, "mono", 600, BONE if j == 0 else TEAL))
b.append(f'<path d="M{W-300} 62 V{H-56}" stroke="{LINE_D}"/>')
b.append(mark(W - 250, 118, 104, ink=BONE, hit=TEAL, op="0.22"))
b.append(T("the bill, not the counter", W - 62, 268, 13, "mono", 400, DIM, 0, "end"))
write("cost-card.svg", svg(W, H, "\n".join(b), "Up to 50% lower whole-task cost in current real-repo tests"))

# ----------------------------------------------------------- social preview
W, H = 1280, 640
b = [f'<rect width="{W}" height="{H}" fill="{INK}"/>']
dots = "".join(f'<circle cx="{x}" cy="{y}" r="2" fill="{BONE}"/>'
               for y in range(40, H, 40) for x in range(40, W, 40))
b.append(f'<g opacity="0.06">{dots}</g>')
b.append(mark(96, 128, 120, ink=BONE, hit=TEAL, op="0.24"))
b.append(T("RepoTracer", 240, 222, 62, "sans", 600, BONE, -0.022))
b.append(T("A read-only repository scout for coding agents.", 96, 336, 34, "sans", 400, BONE, -0.015))
b.append(T("Ask one repo question. RepoTracer searches the codebase", 96, 388, 22, "sans", 400, DIM))
b.append(T("and returns verified file:line references.", 96, 420, 22, "sans", 400, DIM))
b.append(f'<rect x="96" y="472" width="620" height="60" rx="7" fill="{SLATE}"/>')
b.append(T("$", 120, 509, 15, "mono", 500, TEAL))
b.append(T("cargo install repotracer && repotracer setup", 138, 509, 15, "mono", 400, BONE))
b.append(f'<rect x="740" y="472" width="200" height="60" rx="7" fill="none" stroke="{TEAL}"/>')
b.append(T("Search access.", 764, 496, 13, "mono", 500, TEAL))
b.append(T("No write access.", 764, 516, 13, "mono", 400, DIM))
write("social-preview.svg", svg(W, H, "\n".join(b), "RepoTracer — a read-only repository scout for coding agents"))

# ------------------------------------------------------------------- demo card
W, H = 1200, 560
b = [f'<rect width="{W}" height="{H}" fill="{INK}"/>']
b.append('<g opacity="0.055">' + "".join(
    f'<circle cx="{x}" cy="{y}" r="1.8" fill="{BONE}"/>'
    for y in range(30, H, 36) for x in range(30, W, 36)) + '</g>')
b.append(T("›  SAME CODEX PROMPT", 56, 62, 12, "mono", 500, BONE, 0.1))
b.append(T("NO EXTRA PROMPT", W - 56, 62, 12, "mono", 500, TEAL, 0.1, "end"))
b.append(f'<path d="M56 84 H{W-56}" stroke="{LINE_D}"/>')

CX, CWD = 56, W - 112
def card(y, h):
    b.append(f'<rect x="{CX}" y="{y}" width="{CWD}" height="{h}" rx="7" fill="{SLATE}" stroke="{LINE_D}"/>')

card(112, 96)
b.append(T("YOU", CX + 26, 142, 11.5, "mono", 500, DIM, 0.14))
b.append(T("Fix the refresh-token reuse bug and add a regression test.", CX + 26, 180, 20, "mono", 400, BONE))

card(228, 84)
b.append(T("REPOTRACER", CX + 26, 258, 11.5, "mono", 500, TEAL, 0.14))
b.append(T("traced the route in 3.1s", CX + CWD - 26, 258, 11.5, "mono", 400, DIM, 0, "end"))
b.append(f'<rect x="{CX+26}" y="{278}" width="{CWD-52}" height="4" rx="2" fill="{LINE_D}"/>')
b.append(f'<rect x="{CX+26}" y="{278}" width="{(CWD-52)*0.72:.0f}" height="4" rx="2" fill="{TEAL}"/>')

card(332, 134)
b.append(T("4 files to open", CX + 26, 376, 26, "sans", 600, BONE, -0.02))
b.append(T("implementation + state + test", CX + 26, 402, 13, "mono", 400, DIM))
hits = [("src/auth/rotate.rs", ":88-141"), ("src/auth/session.rs", ":12-30"),
        ("src/auth/store.rs", ":204-231"), ("tests/auth_rotation.rs", ":1-64")]
for i, (f, ln) in enumerate(hits):
    hx = CX + 26 + (i % 2) * 420
    hy = 434 + (i // 2) * 24
    b.append(f'<circle cx="{hx+4}" cy="{hy-4}" r="3.2" fill="{TEAL}"/>')
    b.append(T(f, hx + 16, hy, 12.5, "mono", 500, BONE))
    b.append(T(ln, hx + 16 + measure(f, "mono", 500, 12.5), hy, 12.5, "mono", 400, DIM))
b.append(f'<circle cx="{CX+CWD-44}" cy="{376}" r="15" fill="none" stroke="{TEAL}" stroke-width="2"/>')
b.append(f'<path d="M{CX+CWD-51} {376} l5 5 9-10" fill="none" stroke="{TEAL}" stroke-width="2.4" '
         f'stroke-linecap="round" stroke-linejoin="round"/>')

card(486, 0) if False else None
b.append(T("CODEX", CX, 512, 11.5, "mono", 500, DIM, 0.14))
b.append(T("Starts fixing from the right files.", CX + 92, 512, 18, "sans", 400, BONE, -0.01))
b.append(T("RAN AUTOMATICALLY", W - 56, 512, 11.5, "mono", 400, DIM, 0.1, "end"))
write("demo-card.svg", svg(W, H, "\n".join(b), "RepoTracer finds the files Codex needs before Codex starts fixing"))
