#!/usr/bin/env bash
#
# Guards the encoding invariant from AGENTS.md: "FFmpeg is a subprocess, not a
# crate." avz shells out to the system ffmpeg, preflights it with `-version`,
# and pipes raw RGBA to its stdin. Linking an ffmpeg binding instead would drag
# in libav*, break the single-binary promise (VISION.md §2, "no bundled
# FFmpeg"), and make the preflight check meaningless.
#
# The test suite cannot catch this: a swap to a binding crate compiles and
# passes. Only the dependency tree tells the truth.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Skipping ffmpeg-is-a-subprocess check: cargo not found"
    exit 0
fi

# Known Rust bindings to libav*/ffmpeg. --edges normal ignores dev-dependencies.
banned_crates=(
    ac-ffmpeg
    ffmpeg-next
    ffmpeg-sys
    ffmpeg-sys-next
    ffmpeg-the-third
    rsmpeg
    video-rs
)

tree=$(cargo tree --quiet --workspace --edges normal --prefix none | awk '{print $1}')

status=0
for banned in "${banned_crates[@]}"; do
    if grep -qx "${banned}" <<<"${tree}"; then
        echo "error: '${banned}' links ffmpeg into the binary (AGENTS.md: ffmpeg is a subprocess)." >&2
        echo "hint: spawn the system ffmpeg with std::process::Command; see crates/avz-core/src/encode/." >&2
        status=1
    fi
done

if [[ ${status} -eq 0 ]]; then
    echo "ffmpeg stays a subprocess: no ffmpeg binding crates are linked."
fi

exit "${status}"
