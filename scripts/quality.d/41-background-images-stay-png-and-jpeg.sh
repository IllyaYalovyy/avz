#!/usr/bin/env bash
#
# Guards the image scope from VISION.md §13: "Images | image | png/jpg
# backgrounds, cover art".
#
# `background.image` and the embedded cover art are the only images avz reads,
# and both are documented as png or jpeg. The `image` crate's default features
# add gif, webp, tiff, bmp, ico, hdr, pnm, dds, ff, qoi, and exr — a dozen
# decoders in the single binary, each one a parser fed untrusted bytes from
# whatever the user passed to `--bg`.
#
# Dropping `default-features = false` from the workspace manifest is silent: a
# webp background would suddenly render, no test would fail, and the binary
# would grow. Only the dependency tree tells the truth.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Skipping background-image-format check: cargo not found"
    exit 0
fi

# --edges normal ignores dev-dependencies.
tree=$(cargo tree --quiet --workspace --edges normal --prefix none | awk '{print $1}')

# The decoder crates `image` pulls in for formats avz does not read. `png` and
# the jpeg decoder (`zune-jpeg`) are the two that must be there.
forbidden=(
    gif
    image-webp
    ravif
    tiff
    exr
    qoi
    dav1d
)

status=0

for crate in "${forbidden[@]}"; do
    if grep -qx "${crate}" <<<"${tree}"; then
        echo "error: '${crate}' decodes an image format avz does not read (VISION.md §13: png/jpg)." >&2
        echo "hint: keep image at default-features = false, features = [\"jpeg\", \"png\"]." >&2
        status=1
    fi
done

for required in png zune-jpeg; do
    if ! grep -qx "${required}" <<<"${tree}"; then
        echo "error: '${required}' is missing: avz must read the png and jpeg backgrounds it documents." >&2
        status=1
    fi
done

if [[ ${status} -eq 0 ]]; then
    echo "background images stay png and jpeg: no extra image format decoders are linked."
fi

exit "${status}"
