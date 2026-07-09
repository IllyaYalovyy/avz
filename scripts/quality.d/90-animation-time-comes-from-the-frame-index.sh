#!/usr/bin/env bash
#
# Guards determinism (AGENTS.md, VISION.md §5.3): same inputs plus same config
# must produce the same video.
#
# Two ways that promise breaks, both invisible until someone re-renders:
#
#   1. Animation time read from a wall clock instead of `frame_index / fps`.
#      An offline renderer that consults `Instant::now()` produces a different
#      video on a fast machine than on a slow one, and golden frames start to
#      flake for reasons nobody can reproduce.
#   2. Unseeded randomness. Any randomness in avz is a seeded hash of
#      (frame_index, preset seed); a thread RNG is a different video every run.
#
# Neither shows up in a test that renders once. This hook is cheap and catches
# both at the source.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

# Comment lines are excluded so the rule can be documented in the code it
# constrains, exactly as in 10-core-is-ui-agnostic.sh.
offenders=$(
    grep -rnE '\b(Instant::now|SystemTime::now|thread_rng|rng\(\))' crates/avz-core/src |
        grep -vE ':[[:space:]]*//' || true
)

if [[ -n "${offenders}" ]]; then
    echo "error: avz renders deterministically (AGENTS.md, determinism):" >&2
    echo "${offenders}" >&2
    echo "hint: animation time is frame_index / fps, never a wall clock." >&2
    echo "hint: randomness is a seeded hash of (frame_index, preset seed)." >&2
    exit 1
fi

# `rand` would only ever be reached for the unseeded path above: a seeded hash
# needs no crate. --edges normal ignores dev-dependencies.
#
# The crate names are collected before they are searched, never piped into
# `grep -q`: a matching `grep -q` exits at once, awk dies of SIGPIPE, and
# `set -o pipefail` then reports the pipeline as failed — so the `if` below would
# read a *found* `rand` as "not found".
if command -v cargo >/dev/null 2>&1; then
    tree=$(cargo tree --quiet --package avz-core --edges normal --prefix none)
    crates=$(awk '{print $1}' <<<"${tree}")
    if grep -qx "rand" <<<"${crates}"; then
        echo "error: avz-core must not depend on 'rand' (AGENTS.md, determinism)." >&2
        echo "hint: seed a hash with (frame_index, preset seed) instead." >&2
        exit 1
    fi
fi

echo "avz-core renders deterministically: no wall clock, no unseeded RNG."
