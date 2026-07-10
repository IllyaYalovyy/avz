# Bundled fonts

`avz` bundles exactly one font. It is compiled into the binary
(`render::text::BUNDLED_FONT`) and is what `[text] font = "auto"` — the default —
sets the title/artist card in.

## IBM Plex Sans Regular

| | |
|---|---|
| File | `IBMPlexSans-Regular.ttf` |
| Version | 3.005 |
| Upstream | <https://github.com/IBM/plex> (`packages/plex-sans/fonts/complete/ttf/`) |
| Licence | SIL Open Font License 1.1 — `OFL.txt`, copied verbatim from upstream |
| Copyright | © 2017 IBM Corp., with Reserved Font Name "Plex" |

The OFL permits bundling and redistribution, including inside a binary, provided
the licence travels with the font and the Reserved Font Name is not used for a
modified version. `OFL.txt` is that copy. The file here is unmodified.

## Why one font, and why it is the only one

The text card is the one layer whose pixels come from outside this repository.
Letting `cosmic-text` discover the host's fonts would make a render a function of
the machine that ran it, which is exactly what `AGENTS.md` forbids: the same
inputs and the same config must produce the same video. So the font database
holds one face — this one, or the one `[text] font` names — and shaping never
reaches for a fallback.

`scripts/quality.d/42-text-rasterizes-from-the-bundled-font.sh` enforces that,
and checks that this licence file is still here.

Replacing the bundled font is a deliberate, visible change: every committed
`tests/golden/text-card.txt` hash is a hash of these outlines.
