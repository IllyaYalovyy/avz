#!/usr/bin/env bash
#
# Guards the determinism invariant from AGENTS.md: "same inputs plus same config
# must produce the same video".
#
# The text card is the one layer whose pixels come from outside the repository.
# `cosmic-text` will happily find fonts for you: `FontSystem::new()` loads every
# font the host has installed, `Database::load_system_fonts` does it explicitly,
# fontconfig resolves families by rules that differ between distributions, and
# `Shaping::Advanced` falls back to whatever face happens to carry a missing
# glyph. Any one of those makes the rendered card a function of the machine that
# rendered it, and no test would fail — the card would simply be set in a
# different typeface on someone else's laptop.
#
# So: one face in the database, the one we ship or the one `[text] font` names,
# and shaping that never reaches for a second. That is invisible in the diff of
# a `cosmic-text` upgrade, and only the source and the dependency tree tell the
# truth.
#
# It also keeps the licence honest: a bundled font ships with its licence file.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

status=0

fonts="assets/fonts"

if [[ ! -d "${fonts}" ]]; then
    echo "error: ${fonts}/ is missing; the text card needs a bundled font." >&2
    exit 1
fi

if ! compgen -G "${fonts}/*.ttf" >/dev/null && ! compgen -G "${fonts}/*.otf" >/dev/null; then
    echo "error: ${fonts}/ holds no font file." >&2
    status=1
fi

license="${fonts}/OFL.txt"
if [[ ! -f "${license}" ]]; then
    echo "error: ${license} is missing; a bundled font ships with its licence." >&2
    status=1
elif ! grep -q 'SIL OPEN FONT LICENSE' "${license}"; then
    echo "error: ${license} is not the SIL Open Font License." >&2
    echo "hint: avz bundles OFL fonts only. See VISION.md §5.3." >&2
    status=1
fi

# The calls that would let a host font reach a rendered frame.
forbidden=(
    'FontSystem::new()'
    'load_system_fonts'
    'load_fonts_dir'
    'load_font_source'
    'Shaping::Advanced'
)

for call in "${forbidden[@]}"; do
    # Comments are where these calls are explained, not made. Anything else is a
    # call.
    hits=$(grep -rnF --include='*.rs' -- "${call}" crates/ | grep -v ':[[:space:]]*//' || true)
    if [[ -n "${hits}" ]]; then
        echo "error: '${call}' lets the host's fonts into a render." >&2
        echo "hint: build the FontSystem from one face with new_with_locale_and_db," >&2
        echo "      and shape with Shaping::Basic. See render/text.rs." >&2
        echo "${hits}" >&2
        status=1
    fi
done

if command -v cargo >/dev/null 2>&1; then
    # --edges normal ignores dev-dependencies.
    tree=$(cargo tree --quiet --workspace --edges normal --prefix none | awk '{print $1}')

    if grep -qx 'fontconfig-parser' <<<"${tree}"; then
        echo "error: 'fontconfig-parser' is linked: the card would be set in whatever" >&2
        echo "       font this host's fontconfig resolves." >&2
        echo "hint: keep cosmic-text at default-features = false." >&2
        status=1
    fi
else
    echo "Skipping the dependency-tree half of this check: cargo not found"
fi

if [[ ${status} -eq 0 ]]; then
    echo "text rasterizes from the bundled font: no host fonts reach a render."
fi

exit "${status}"
