#!/usr/bin/env bash
#
# Guards the golden-frame rule from AGENTS.md and docs/TESTING.md: "Golden-frame
# tests run on the software adapter only, because GPU float differences are
# expected across machines."
#
# A golden test that opened `AdapterChoice::Auto` would hash whatever adapter the
# developer happened to have. It passes on their machine, fails everywhere else,
# and the committed hashes become a hash of one laptop. The failure never looks
# like a shader regression, which is the only thing golden frames exist to catch.
#
# Cheap to check, invisible otherwise: the harness names its adapter in source.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

readonly harness="crates/avz-core/tests/golden_frames.rs"
readonly hashes="crates/avz-core/tests/golden"

status=0

if [[ ! -f ${harness} ]]; then
    echo "error: ${harness} is missing; nothing pins the shaders." >&2
    exit 1
fi

if ! grep -q 'AdapterChoice::Software' "${harness}"; then
    echo "error: ${harness} never asks for the software adapter." >&2
    echo "hint: golden frames render on lavapipe, by name, always." >&2
    status=1
fi

# Comment lines are excluded so the rule can be documented in the code it
# constrains, exactly as in 10-core-is-ui-agnostic.sh.
offenders=$(
    grep -nE 'AdapterChoice::(Auto|Gpu)' "${harness}" |
        grep -vE ':[[:space:]]*//' || true
)

if [[ -n ${offenders} ]]; then
    echo "error: a golden-frame test may render on a hardware adapter:" >&2
    echo "${offenders}" >&2
    echo "hint: use AdapterChoice::Software; GPU float differences are expected." >&2
    status=1
fi

# A preset with no committed hashes is a preset nothing protects.
while IFS= read -r preset; do
    name=$(basename "${preset}" .wgsl)
    if [[ ! -f "${hashes}/${name}.txt" ]]; then
        echo "error: preset '${name}' has no golden hashes at ${hashes}/${name}.txt" >&2
        echo "hint: AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core --test golden_frames" >&2
        status=1
    fi
done < <(find crates/avz-core/presets -maxdepth 1 -name '*.wgsl' | sort)

if [[ ${status} -eq 0 ]]; then
    echo "Golden frames render on the software adapter, and every preset has hashes."
fi

exit "${status}"
