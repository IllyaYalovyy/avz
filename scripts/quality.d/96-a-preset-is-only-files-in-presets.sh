#!/usr/bin/env bash
#
# Guards RFC-001 G3: "adding a 4th preset requires touching only `presets/`."
#
# That promise is what buys the four deferred presets (RFC-001 NG1) their
# cheapness, and it is invisible until someone adds one and finds themselves
# editing the renderer. The mechanism is `presets/registry.rs`, `include!`d by
# `src/render/preset.rs`: the registry rows, the WGSL, and the JSON schemas all
# live in `presets/`, so a new preset is three files in one directory.
#
# The three ways that erodes, all cheap to check:
#   1. the registry drifts back into `src/`,
#   2. a shader ships without the schema `avz presets` and `--set` validate on,
#   3. a schema or shader sits in the directory that no registry row names.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

readonly presets="crates/avz-core/presets"
readonly registry="${presets}/registry.rs"
readonly module="crates/avz-core/src/render/preset.rs"

status=0

if [[ ! -f ${registry} ]]; then
    echo "error: ${registry} is missing; the preset registry must live in ${presets}." >&2
    exit 1
fi

if ! grep -q 'include!("../../presets/registry.rs")' "${module}"; then
    echo "error: ${module} no longer includes ${registry}." >&2
    echo "hint: the registry lives in presets/ so a new preset touches nothing else." >&2
    status=1
fi

# A `PRESETS` row, or an `include_str!` of a preset asset, outside `presets/`
# means the next preset author edits `src/`. Comment lines are excluded so the
# rule can be documented in the code it constrains.
offenders=$(
    grep -rnE 'include_str!\("[^"]*\.(wgsl|json)"\)|PRESETS:[[:space:]]*&\[' \
        crates/avz-core/src crates/avz-cli/src |
        grep -vE ':[[:space:]]*(//|#)' || true
)
if [[ -n ${offenders} ]]; then
    echo "error: a preset is embedded or registered outside ${presets}:" >&2
    echo "${offenders}" >&2
    echo "hint: add the row to ${registry} instead." >&2
    status=1
fi

while IFS= read -r shader; do
    name=$(basename "${shader}" .wgsl)

    if [[ ! -f "${presets}/${name}.json" ]]; then
        echo "error: preset '${name}' has no schema at ${presets}/${name}.json" >&2
        echo "hint: a preset is a WGSL file plus a JSON parameter schema (VISION.md §6)." >&2
        status=1
    fi

    if ! grep -q "\"${name}\"" "${registry}"; then
        echo "error: preset '${name}' is not registered in ${registry}" >&2
        status=1
    fi
done < <(find "${presets}" -maxdepth 1 -name '*.wgsl' | sort)

# And the other way round: a schema with no shader is a schema nothing reads.
while IFS= read -r schema; do
    name=$(basename "${schema}" .json)
    if [[ ! -f "${presets}/${name}.wgsl" ]]; then
        echo "error: schema '${name}' has no shader at ${presets}/${name}.wgsl" >&2
        status=1
    fi
done < <(find "${presets}" -maxdepth 1 -name '*.json' | sort)

if [[ ${status} -eq 0 ]]; then
    echo "Every preset is a shader, a schema, and one row in presets/registry.rs."
fi

exit "${status}"
