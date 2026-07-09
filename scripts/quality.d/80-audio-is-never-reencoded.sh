#!/usr/bin/env bash
#
# Guards the audio invariant from AGENTS.md: "Never re-encode the audio. The
# original mp3 stream is muxed with `-c:a copy`."
#
# A re-encode is invisible: the video plays, the audio sounds fine, and every
# render loses a generation of quality. `muxed_audio_stream_is_copied_not_
# reencoded` catches it by asking ffprobe what landed in the container, but that
# test needs ffmpeg installed. This hook reads the source instead, so a
# `-c:a aac` typed into the argv is caught even when the encoder cannot run.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

readonly home="crates/avz-core/src/encode/encoder.rs"

if [[ ! -f ${home} ]]; then
    echo "error: ${home} is missing; the argv that muxes the audio has no home." >&2
    exit 1
fi

# Test modules name the encoders they forbid, so they must not be scanned. Every
# `#[cfg(test)]` block in this crate sits at the bottom of its file.
strip_tests() {
    awk '/^#\[cfg\(test\)\]/ { exit } { print }' "$1"
}

status=0

# Encoders that would replace the mp3 stream. `-b:a` and `-ar` only make sense
# when something is being encoded, so they are equally disqualifying.
banned='libmp3lame|libfdk_aac|libopus|libvorbis|libmp3|"aac"|"-b:a"|"-ar"|"-ac"'

while IFS= read -r source; do
    offense=$(strip_tests "${source}" | grep -nE "${banned}" || true)
    if [[ -n ${offense} ]]; then
        echo "error: ${source} names an audio encoder:" >&2
        echo "${offense}" >&2
        status=1
    fi

    # `-c:a` may only ever be followed by `copy`.
    stray=$(
        strip_tests "${source}" |
            awk '
                expecting { if ($0 !~ /"copy"/) { print NR": "$0 }; expecting = 0; next }
                /"-c:a"/  { expecting = 1 }
            ' || true
    )
    if [[ -n ${stray} ]]; then
        echo "error: ${source} passes -c:a something other than copy:" >&2
        echo "${stray}" >&2
        status=1
    fi
done < <(find crates -path '*/src/*' -name '*.rs' -type f | sort)

# The invariant is only guarded if the flag is actually there to guard.
#
# The argv is collected before it is searched, never piped into `grep -q`: a
# matching `grep -q` exits at once, awk dies of SIGPIPE, and `set -o pipefail`
# then reports the pipeline as failed — which made this check fail at random on
# a source file that does contain `-c:a`.
argv=$(strip_tests "${home}")

if ! grep -q '"-c:a"' <<<"${argv}"; then
    echo "error: ${home} no longer passes -c:a to ffmpeg." >&2
    echo "hint: the mp3 stream must be muxed with \`-c:a copy\` (AGENTS.md)." >&2
    status=1
fi

if [[ ${status} -eq 0 ]]; then
    echo "Audio is muxed with -c:a copy: no encoder re-encodes the mp3 stream."
else
    echo "hint: avz muxes the original mp3 untouched; see ${home}." >&2
fi

exit "${status}"
