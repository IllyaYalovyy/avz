#!/usr/bin/env bash
#
# Guards assets/fixtures/ from the two ways a test fixture goes wrong:
#
#   1. Someone drops a real song in. The repo then ships audio it has no licence
#      to redistribute, and `git clone` starts costing megabytes.
#   2. Someone regenerates a fixture from a source that is not CC0 and forgets to
#      say so. The provenance note in the README is the only record we keep.
#
# Neither failure can be caught by a test: a copyrighted 8 MB mp3 makes the suite
# pass. Only the bytes on disk and the licence note tell the truth.
#
# Regenerate fixtures with ./scripts/make-test-fixture.sh.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

fixtures="assets/fixtures"

if [[ ! -d "${fixtures}" ]]; then
    echo "Skipping fixture check: ${fixtures}/ does not exist"
    exit 0
fi

# A few seconds of 64 kbps mono/stereo tones is ~40 KB. 256 KiB leaves room to
# grow without leaving room for a track.
readonly MAX_BYTES=$((256 * 1024))

status=0

readme="${fixtures}/README.md"
if [[ ! -f "${readme}" ]]; then
    echo "error: ${readme} is missing; fixtures must record their provenance." >&2
    status=1
elif ! grep -q 'CC0' "${readme}"; then
    echo "error: ${readme} must state the CC0 dedication for the fixtures." >&2
    echo "hint: fixtures are authored by this project and dedicated to the public domain." >&2
    status=1
fi

while IFS= read -r -d '' file; do
    bytes=$(wc -c <"${file}")
    if ((bytes > MAX_BYTES)); then
        echo "error: ${file} is ${bytes} bytes, over the ${MAX_BYTES}-byte fixture ceiling." >&2
        echo "hint: fixtures are synthesized tones, not music. See ${readme}." >&2
        status=1
    fi
done < <(find "${fixtures}" -type f ! -name 'README.md' -print0)

if [[ ${status} -eq 0 ]]; then
    echo "test fixtures stay small and CC0-dedicated."
fi

exit "${status}"
