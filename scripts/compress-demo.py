#!/usr/bin/env python3
"""Fast-forward the idle wait in a recorded demo GIF.

The scout genuinely takes ~35s. Showing that unedited makes a poor hero loop, so
this keeps typing and results at real speed and only compresses the dead wait.
Output frames are never altered, so the terminal content stays truthful.

Usage: python3 scripts/compress-demo.py assets/demo/scout.gif
"""
import subprocess, sys, tempfile, glob, shutil, os
from pathlib import Path

TYPE_END, RESULT, WAIT_KEEP, HOLD = 32, 207, 10, 45

def main(gif: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        src, dst = Path(tmp) / "src", Path(tmp) / "dst"
        src.mkdir(); dst.mkdir()
        subprocess.run(["ffmpeg", "-y", "-i", gif, "-vsync", "0", f"{src}/f%04d.png"], check=True, capture_output=True)
        fs = sorted(glob.glob(f"{src}/f*.png"))
        out = fs[:TYPE_END] + fs[TYPE_END:RESULT:WAIT_KEEP] + fs[RESULT:] + [fs[-1]] * HOLD
        for i, f in enumerate(out):
            shutil.copy(f, dst / f"r{i:04d}.png")
        subprocess.run([
            "ffmpeg", "-y", "-framerate", "25", "-i", f"{dst}/r%04d.png",
            "-vf", "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3",
            "-loop", "0", gif,
        ], check=True, capture_output=True)
        print(f"{gif}: {len(fs)} -> {len(out)} frames")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "assets/demo/scout.gif")
