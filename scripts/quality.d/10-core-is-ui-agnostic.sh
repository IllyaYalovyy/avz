#!/usr/bin/env bash
#
# Guards the core/cli split from AGENTS.md: avz-core stays UI-agnostic, so a GUI
# or batch orchestrator can be layered on later without refactoring.
#
# Two invariants:
#   1. avz-core performs no terminal I/O. Progress goes through the Progress
#      callback trait; all printing lives in avz-cli.
#   2. avz-core does not depend on UI-layer crates. anyhow provides error
#      context chains at the CLI boundary only; core errors stay typed.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Skipping core/cli split check: cargo not found"
    exit 0
fi

status=0

# 1. No terminal I/O. Comment lines are excluded so the rule can be documented
#    in the very crate it constrains.
io_offenders=$(
    grep -rnE '\b(println|eprintln|print|eprint|dbg)!' crates/avz-core/src |
        grep -vE ':[[:space:]]*//' || true
)

if [[ -n "${io_offenders}" ]]; then
    echo "error: avz-core must not perform terminal I/O (AGENTS.md, core/cli split):" >&2
    echo "${io_offenders}" >&2
    echo "hint: report progress through the Progress trait; print from avz-cli." >&2
    status=1
fi

# 2. No UI-layer dependencies. --edges normal ignores dev-dependencies, which
#    are free to pull in whatever a test needs.
#
#    The crate names are collected before they are searched, never piped into
#    `grep -q`: a matching `grep -q` exits at once, awk dies of SIGPIPE, and
#    `set -o pipefail` then reports the pipeline as failed — so the `if` below
#    would read a *found* banned crate as "not found".
tree=$(cargo tree --quiet --package avz-core --edges normal --prefix none)
crates=$(awk '{print $1}' <<<"${tree}")

for banned in clap anyhow indicatif; do
    if grep -qx "${banned}" <<<"${crates}"; then
        echo "error: avz-core must not depend on '${banned}' (AGENTS.md, core/cli split)." >&2
        echo "hint: keep '${banned}' in avz-cli; core uses typed thiserror errors." >&2
        status=1
    fi
done

if [[ ${status} -eq 0 ]]; then
    echo "avz-core is UI-agnostic: no terminal I/O, no UI-layer dependencies."
fi

exit "${status}"
