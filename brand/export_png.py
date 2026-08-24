#!/usr/bin/env python3
"""Rasterise the brand SVGs with headless Brave (transparent background)."""
import pathlib, subprocess, tempfile
BRAVE = "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
A = pathlib.Path("assets").resolve()
JOBS = [("logo-mark.svg","logo-mark-512.png",512,512),
        ("logo-mark-inverse.svg","logo-mark-inverse-512.png",512,512),
        ("logo-lockup.svg","logo-lockup.png",1000,None),
        ("logo-lockup-inverse.svg","logo-lockup-inverse.png",1000,None),
        ("favicon.svg","favicon-512.png",512,512),
        ("favicon.svg","favicon-180.png",180,180),
        ("favicon.svg","favicon-32.png",32,32),
        ("social-preview.svg","social-preview.png",1280,640),
        ("cost-card.svg","cost-card.png",1200,340),
        ("readme-flow.svg","readme-flow.png",1200,452)]
import re
for src, dst, w, h in JOBS:
    svg = (A/src).read_text()
    vw, vh = [float(v) for v in re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', svg).groups()]
    h = h or round(w * vh / vw)
    html = f'<body style="margin:0"><img src="file://{A/src}" style="width:{w}px;height:{h}px;display:block"></body>'
    with tempfile.NamedTemporaryFile("w", suffix=".html", delete=False) as f:
        f.write(html); page = f.name
    subprocess.run([BRAVE,"--headless","--disable-gpu","--hide-scrollbars",
                    "--default-background-color=00000000",
                    f"--screenshot={A/dst}", f"--window-size={w},{h}", page],
                   capture_output=True)
    print(f"assets/{dst}  {w}x{h}  {(A/dst).stat().st_size//1024}KB")
