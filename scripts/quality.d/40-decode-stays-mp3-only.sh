#!/usr/bin/env bash
#
# Guards the decode scope from VISION.md §13: "Decode | symphonia | mp3 feature".
# avz takes an mp3 in and copies that same mp3 stream out (`-c:a copy`), so
# decoding anything else would be a format avz could never mux back.
#
# symphonia's default features pull in flac, ogg, vorbis, wav, pcm, and adpcm.
# Dropping `default-features = false` from the workspace manifest silently grows
# the single binary and starts half-supporting formats the rest of the pipeline
# rejects. The test suite cannot catch it: everything still compiles and passes.
# Only the dependency tree tells the truth.
#
# The mp3 decoder ships as symphonia-bundle-mp3; symphonia-metadata is the
# `id3v2` reader that lets the demuxer skip the tag `probe` already read.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Skipping decode-stays-mp3-only check: cargo not found"
    exit 0
fi

# --edges normal ignores dev-dependencies.
tree=$(cargo tree --quiet --workspace --edges normal --prefix none | awk '{print $1}')

allowed_symphonia=(
    symphonia
    symphonia-bundle-mp3
    symphonia-core
    symphonia-metadata
)

status=0

while IFS= read -r crate; do
    for allowed in "${allowed_symphonia[@]}"; do
        if [[ "${crate}" == "${allowed}" ]]; then
            continue 2
        fi
    done

    echo "error: '${crate}' decodes a format avz does not read (VISION.md §13: mp3 only)." >&2
    echo "hint: keep symphonia at default-features = false, features = [\"mp3\", \"id3v2\"]." >&2
    status=1
done < <(grep '^symphonia' <<<"${tree}" | sort -u)

if [[ ${status} -eq 0 ]]; then
    echo "decode stays mp3-only: no extra symphonia format or codec crates are linked."
fi

exit "${status}"
