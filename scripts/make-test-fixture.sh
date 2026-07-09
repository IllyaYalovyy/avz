#!/usr/bin/env bash
#
# Regenerate the committed test fixtures in assets/fixtures/.
#
# RFC-001 Q1 asked which CC0 mp3 becomes the repo fixture. No suitable file
# existed with both ID3v2 tags and embedded cover art, so avz authors its own:
# synthesized tones and a generated gradient, both dedicated to the public
# domain under CC0-1.0. Nothing here is copied from anywhere.
#
# The audio is deliberately not a flat tone. A 60 Hz kick decaying every 500 ms
# under a steady 1 kHz tone gives the M1 tracer bullet something to react to:
# loudness visibly rises and falls, and the bass band is separable from the mid.
#
# Output is bit-exact given the same ffmpeg and libmp3lame, so regenerating
# without changing the recipe produces no diff.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixtures="${repo_root}/assets/fixtures"

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "error: ffmpeg is required to regenerate fixtures." >&2
    echo "hint: sudo dnf install ffmpeg" >&2
    exit 1
fi

readonly TITLE="Sine Tones"
readonly ARTIST="avz test fixture"
readonly ALBUM="Public Domain Tones"

# 60 Hz kick decaying every 500 ms, plus a 1 kHz tone breathing once a second.
readonly TONES="0.55*sin(2*PI*60*t)*exp(-6*mod(t,0.5))+0.18*sin(2*PI*1000*t)*(0.4+0.6*exp(-3*mod(t,1)))"

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

mkdir -p "${fixtures}"

echo "==> Synthesizing 5 s of tones"
ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "aevalsrc='${TONES}':c=stereo:s=44100:d=5" \
    -c:a libmp3lame -b:a 64k -ar 44100 -ac 2 \
    "${work}/tone.mp3"

echo "==> Generating 256x256 cover art"
ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "gradients=s=256x256:c0=0x1a1a2e:c1=0xe94560:x0=0:y0=0:x1=255:y1=255:d=1:seed=1" \
    -frames:v 1 "${work}/cover.png"

# -map_metadata -1 and -fflags +bitexact drop ffmpeg's own encoder/version tags,
# which would otherwise churn the committed bytes on every ffmpeg upgrade.
echo "==> Writing tone-tagged.mp3 (ID3v2.3 tags + APIC cover art)"
ffmpeg -hide_banner -loglevel error -y \
    -i "${work}/tone.mp3" -i "${work}/cover.png" \
    -map 0:a -map 1:v -c:a copy -c:v png -disposition:v:0 attached_pic \
    -map_metadata -1 -fflags +bitexact -flags:v +bitexact -flags:a +bitexact \
    -id3v2_version 3 -write_id3v1 0 \
    -metadata title="${TITLE}" \
    -metadata artist="${ARTIST}" \
    -metadata album="${ALBUM}" \
    "${fixtures}/tone-tagged.mp3"

echo "==> Writing tone-untagged.mp3 (no ID3 header at all)"
ffmpeg -hide_banner -loglevel error -y \
    -i "${work}/tone.mp3" -map 0:a -c:a copy \
    -map_metadata -1 -fflags +bitexact -flags:a +bitexact \
    -id3v2_version 0 -write_id3v1 0 \
    "${fixtures}/tone-untagged.mp3"

echo
ls -l "${fixtures}"
