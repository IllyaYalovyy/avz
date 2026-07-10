#!/usr/bin/env bash
#
# The v0.1 acceptance test: batch-render an album unattended (UT-010, VISION §9 M5).
#
#   scripts/album-acceptance.sh [ALBUM_DIR] [CONFIG]
#
# Runs the loop VISION.md §3 ships instead of a `batch` subcommand:
#
#   for f in album/*.mp3; do avz render "$f" --config album.toml; done
#
# and reports what a release note has to state: track count, adapter, wall time
# per track, every warning, and whether anything needed a human. An intervention
# is any non-zero exit, any missing mp4, and any `.part` file left on disk — the
# run fails if it sees one.
#
# With no ALBUM_DIR it synthesizes a four-track stand-in album with ffmpeg: a
# quiet track, a dense one, a sparse one, and a silent one, tagged and of
# different lengths. That is a repeatable gate, not a substitute for the manual
# listening pass on real music (docs/TESTING.md) — "feels musical" has no
# assertion, and a sine wave cannot answer it.
#
# `AVZ` names the binary to test; it defaults to the release build in this tree.
# Point it at an installed one to verify a `cargo install`:
#
#   AVZ=~/.cargo/bin/avz scripts/album-acceptance.sh
#
# This is minutes of rendering, so it is not a `scripts/quality.d/` hook. The
# part of it that fits in the gate is `an_album_batch_renders_every_song_to_its_own_mp4_unattended`
# in `crates/avz-cli/tests/render_e2e.rs`.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

AVZ=${AVZ:-${repo_root}/target/release/avz}
ADAPTER=${ADAPTER:-auto}

album=${1:-}
config=${2:-}

if [[ ! -x ${AVZ} ]]; then
    echo "error: no avz binary at ${AVZ}" >&2
    echo "hint: cargo build --release -p avz-cli, or set AVZ=<path>" >&2
    exit 1
fi

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "error: ffmpeg is required." >&2
    echo "hint: sudo dnf install ffmpeg" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

# A stand-in album: four tracks that stress different corners of the analysis —
# a kick under a tone, a dense chord, a sparse click track, and digital silence
# (whose p5..p95 spread is degenerate and must normalize to zeros, not NaNs).
synthesize_album() {
    local dir=$1
    mkdir -p "${dir}"

    local -a names=(01-opener 02-dense 03-sparse 04-silence)
    local -a exprs=(
        "0.55*sin(2*PI*60*t)*exp(-6*mod(t,0.5))+0.18*sin(2*PI*1000*t)"
        "0.30*sin(2*PI*110*t)+0.25*sin(2*PI*220*t)+0.20*sin(2*PI*440*t)+0.15*sin(2*PI*3000*t)*random(0)"
        "0.80*exp(-40*mod(t,0.75))*random(1)"
        "0"
    )
    local -a lengths=(20 25 20 15)

    local index
    for index in "${!names[@]}"; do
        ffmpeg -hide_banner -loglevel error -y \
            -f lavfi -i "aevalsrc='${exprs[index]}':c=stereo:s=44100:d=${lengths[index]}" \
            -c:a libmp3lame -b:a 128k -ar 44100 -ac 2 \
            -metadata title="${names[index]}" \
            -metadata artist="avz acceptance" \
            -metadata album="Synthetic Album" \
            -id3v2_version 3 \
            "${dir}/${names[index]}.mp3"
    done
}

synthetic=no
if [[ -z ${album} ]]; then
    synthetic=yes
    album="${work}/album"
    echo "==> No album given; synthesizing a four-track stand-in"
    synthesize_album "${album}"
fi

if [[ ! -d ${album} ]]; then
    echo "error: ${album} is not a directory" >&2
    exit 1
fi

if [[ -z ${config} ]]; then
    config="${work}/album.toml"
    "${AVZ}" config --example >"${config}"
    echo "==> No config given; using \`avz config --example\`"
fi

shopt -s nullglob
tracks=("${album}"/*.mp3)
shopt -u nullglob

if [[ ${#tracks[@]} -eq 0 ]]; then
    echo "error: no mp3s in ${album}" >&2
    exit 1
fi

echo "==> Album: ${album} (${#tracks[@]} tracks, synthetic=${synthetic})"
echo "==> Config: ${config}"
echo "==> Binary: ${AVZ} ($("${AVZ}" --version))"
echo "==> Adapter: ${ADAPTER}"
echo

warnings="${work}/warnings.txt"
: >"${warnings}"

interventions=0
run_start=$(date +%s)

# The loop, exactly as `VISION.md` §3 spells it. Nothing here retries, prompts,
# or cleans up after a failure: a batch that needs a human is a failed batch.
for track in "${tracks[@]}"; do
    stderr="${work}/$(basename "${track}").stderr"
    start=$(date +%s)

    if "${AVZ}" render "${track}" --config "${config}" --adapter "${ADAPTER}" \
        >/dev/null 2>"${stderr}"; then
        status=ok
    else
        status="FAILED (exit $?)"
        interventions=$((interventions + 1))
    fi

    elapsed=$(($(date +%s) - start))
    printf '  %-24s %-16s %3ds\n' "$(basename "${track}")" "${status}" "${elapsed}"

    # Warnings are the other half of the report: a run with zero interventions
    # that nagged four times is a run whose defaults are wrong.
    if grep -i 'warning' "${stderr}" >>"${warnings}"; then :; fi
    if [[ ${status} != ok ]]; then
        sed 's/^/      /' "${stderr}" >&2
    fi
done

run_elapsed=$(($(date +%s) - run_start))
echo
echo "==> Wall time: ${run_elapsed}s for ${#tracks[@]} tracks"

# Every track produced its own playable mp4, and nothing half-written survives.
for track in "${tracks[@]}"; do
    output="${track%.mp3}.mp4"
    if [[ ! -f ${output} ]]; then
        echo "  missing output: ${output}" >&2
        interventions=$((interventions + 1))
        continue
    fi

    streams=$(ffprobe -v error -show_entries stream=codec_type -of csv=p=0 "${output}" | sort | tr '\n' ' ')
    if [[ ${streams} != "audio video " ]]; then
        echo "  ${output}: expected one audio and one video stream, got: ${streams}" >&2
        interventions=$((interventions + 1))
    fi
done

shopt -s nullglob
leftovers=("${album}"/*.part)
shopt -u nullglob
if [[ ${#leftovers[@]} -gt 0 ]]; then
    echo "  half-written files left behind: ${leftovers[*]}" >&2
    interventions=$((interventions + 1))
fi

if [[ -s ${warnings} ]]; then
    echo "==> Warnings:"
    sort -u "${warnings}" | sed 's/^/  /'
else
    echo "==> Warnings: none"
fi

echo
if [[ ${interventions} -eq 0 ]]; then
    echo "==> Acceptance PASSED: ${#tracks[@]} tracks, zero interventions"
else
    echo "==> Acceptance FAILED: ${interventions} intervention(s)" >&2
    exit 1
fi
