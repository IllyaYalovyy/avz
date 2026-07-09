#!/usr/bin/env bash
#
# Guards the readback invariant from AGENTS.md: "Keep the 256-byte readback
# row-padding handling in exactly one place."
#
# wgpu pads every texture-to-buffer copy row up to COPY_BYTES_PER_ROW_ALIGNMENT.
# A second place that reasons about that padding is how a frame ends up sheared
# by 80 bytes per row — output that looks plausible and is silently wrong. The
# one place is render/readback.rs, whose RowLayout owns both the arithmetic and
# the unpadding copy.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

readonly home="crates/avz-core/src/render/readback.rs"

if [[ ! -f ${home} ]]; then
    echo "error: ${home} is missing; the padding invariant has no home." >&2
    exit 1
fi

status=0

# The alignment constant, and the stride derived from it, may only be named in
# readback.rs. Everywhere else must go through RowLayout.
offenders=$(
    grep -rln -e 'COPY_BYTES_PER_ROW_ALIGNMENT' -e 'ROW_ALIGNMENT' \
        crates/*/src crates/*/tests |
        grep -vFx "${home}" || true
)

if [[ -n ${offenders} ]]; then
    echo "error: the 256-byte row alignment is named outside ${home}:" >&2
    echo "${offenders}" >&2
    echo "hint: use render::readback::RowLayout; it owns the padding math." >&2
    status=1
fi

# padded_bytes_per_row is RowLayout's to compute. Call it anywhere, define it
# once: a second definition is a second source of truth.
definitions=$(
    grep -rln 'fn padded_bytes_per_row' crates/*/src |
        grep -vFx "${home}" || true
)

if [[ -n ${definitions} ]]; then
    echo "error: padded_bytes_per_row is defined outside ${home}:" >&2
    echo "${definitions}" >&2
    echo "hint: one definition, in RowLayout. Call it, do not recompute it." >&2
    status=1
fi

if [[ ${status} -eq 0 ]]; then
    echo "Readback row padding lives only in ${home}."
fi

exit "${status}"
