#!/usr/bin/env bash
#
# Guards the codec matrix: avz names an ffmpeg encoder in exactly one place.
#
# `--codec x264|x265|av1` is avz's spelling; `libx264|libx265|libsvtav1` is
# ffmpeg's, and the translation between them lives in `video_encoder`. A second
# place that hard-codes `libx265` is how `--codec av1` ends up writing an h264
# file, or how the availability check passes for an encoder `Encoder::start`
# then fails on — both of which look like a working render right up until the
# file is played.
#
# The tests can only see the encoders the machine running them happens to have
# (Fedora's stock `ffmpeg-free` has neither x264 nor x265), so they skip what
# they cannot exercise. This hook reads the source instead and skips nothing.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

readonly home="crates/avz-core/src/encode/encoder.rs"

if [[ ! -f ${home} ]]; then
    echo "error: ${home} is missing; the codec matrix has no home." >&2
    exit 1
fi

# The ffmpeg encoder names avz may emit, and the AV1 encoders it deliberately
# does not: naming libaom or rav1e would mean two AV1 code paths.
readonly encoders='libx264|libx265|libsvtav1'
readonly rejected='libaom-av1|librav1e|av1_nvenc|av1_qsv|av1_vaapi|libx264rgb'

status=0

# Test modules name the encoders they assert on, so they must not be scanned.
# Every `#[cfg(test)]` block in this crate sits at the bottom of its file.
strip_tests() {
    awk '/^#\[cfg\(test\)\]/ { exit } { print }' "$1"
}

offenders=$(
    while IFS= read -r source; do
        if [[ ${source} == "${home}" ]]; then
            continue
        fi
        offense=$(strip_tests "${source}" | grep -nE "\"(${encoders})\"" || true)
        if [[ -n ${offense} ]]; then
            echo "${source}: ${offense}"
        fi
    done < <(find crates -path '*/src/*' -name '*.rs' -type f | sort)
)

if [[ -n ${offenders} ]]; then
    echo "error: an ffmpeg encoder is named outside ${home}:" >&2
    echo "${offenders}" >&2
    echo "hint: call encode::video_encoder(codec); it owns the translation." >&2
    status=1
fi

matrix=$(strip_tests "${home}")

# The matrix is only guarded if it is actually there to guard. One `-c:v` per
# codec, and the pixel format that keeps every one of them broadly playable.
for encoder in libx264 libx265 libsvtav1; do
    if ! grep -q "\"${encoder}\"" <<<"${matrix}"; then
        echo "error: ${home} no longer names the \`${encoder}\` encoder." >&2
        echo "hint: every Codec variant maps to one ffmpeg encoder (VISION.md 5.4)." >&2
        status=1
    fi
done

if ! grep -q '"yuv420p"' <<<"${matrix}"; then
    echo "error: ${home} no longer forces -pix_fmt yuv420p." >&2
    echo "hint: it is the compatibility promise, and it holds across the matrix." >&2
    status=1
fi

stray=$(grep -nE "\"(${rejected})\"" <<<"${matrix}" || true)
if [[ -n ${stray} ]]; then
    echo "error: ${home} names an encoder avz does not ship:" >&2
    echo "${stray}" >&2
    echo "hint: one encoder per codec. AV1 is SVT-AV1 (VISION.md 5.4)." >&2
    status=1
fi

if [[ ${status} -eq 0 ]]; then
    echo "The codec matrix lives only in ${home}: x264, x265, av1, all yuv420p."
fi

exit "${status}"
