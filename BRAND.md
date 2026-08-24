# RepoTracer brand

Small. The whole system is four SVGs, three colors, and two typefaces.

## Name

**RepoTracer** in prose. `repotracer` in code and as the binary. Never "Repo Tracer", never "RT".

The name is literal, which is the point: it traces a repository and returns where things are.

## The mark

![RepoTracer mark](./assets/logo-mark.svg)

A bracket enclosing three bars of decreasing width. The shortest bar is the accent color and sits indented.

It reads two ways, both true: lines of code narrowing to one result, and a search filtering down to the single location that matters. The bracket is the repository boundary — the scout works inside it and never writes outside it.

**Construction.** 32×32 viewBox. Bracket stroke 2.6, round caps and joins, from `(11.4,7)` to `(8.2,25)`. Bars are 3.6 tall with `rx` 1.8, at y = 9.1 / 14.2 / 19.3, widths 15.4 / 10.2 / 6.6. The third bar starts at x 15.6; the first two start at 13.4.

**Rules.**

- The short bar is always the accent. It is the only chroma in the mark.
- Never recolor the bracket or the long bars.
- Never reorder the bars. They always shorten downward.
- Clear space on all sides equals one bar height (3.6 units, ~11% of the mark's width).
- Below 20px use `favicon.svg`, which drops the tagline and thickens the strokes.

**Do not add:** magnifying glasses, dot matrices, paw prints, brains, neural nets, sparkles, or gradients.

### Files

| File | Use |
|---|---|
| `assets/logo-mark.svg` | Primary mark, light grounds |
| `assets/logo-mark-inverse.svg` | Dark grounds |
| `assets/logo-mark-mono.svg` | `currentColor` throughout — inherits text color |
| `assets/logo-lockup.svg` | Horizontal mark + wordmark |
| `assets/logo-lockup-stacked.svg` | Centered mark, wordmark, tagline |
| `assets/favicon.svg` | Rounded tile, 20px and below |

## Color

Three values carry the whole identity.

| Token | Hex | Role |
|---|---|---|
| Cream | `#f5f3eb` | Page ground. Warm, never pure white. |
| Ink | `#0e3d42` | Type, mark, dark bands. |
| Accent | `#e4ee4f` | One element per view. Never a background for body text. |

Supporting values: `#efece1` raised surfaces, `#0a2e33` and `#04252b` terminal grounds, `#5b7276` secondary text, `#d9d5c6` hairlines.

The accent is a highlighter, not a brand color. If two things on a screen are accent-colored, one of them is wrong.

## Type

**DM Sans** for everything, **JetBrains Mono** for code and terminal output.

| Role | Size | Weight | Tracking | Leading |
|---|---|---|---|---|
| Hero | `clamp(42px, 6vw, 82px)` | 700 | −0.032em | 1.04 |
| Section | `clamp(28px, 3.4vw, 40px)` | 700 | −0.03em | 1.1 |
| Body | 17px | 400 | normal | 1.6 |
| Lede | 18px | 400 | normal | 1.6 |

Headlines are tight and set narrow — cap the hero at roughly 18 characters per line so it wraps to two. Sentence case with a terminal period. Never all-caps a headline.

## Voice

Say what the thing is before saying why it's good. Every number traces to a checksummed artifact in `benchmarks/results/`, and the losing runs get published next to the wins.

Banned: "tokens saved" as a headline claim, "AI-powered", "supercharge", "10x", "revolutionize", "seamlessly", and any cost figure that isn't in a benchmark artifact.

## Implementation

Tokens are defined in `style.css` under `:root`. `brand/tokens.css` and the `brand/build_*.py` scripts generated the **retired** dot-matrix mark and are kept only as history — do not regenerate assets from them.
