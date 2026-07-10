#!/usr/bin/env bash
#
# Guards the CLI invariant from AGENTS.md: "Warnings must be actionable and say
# what to do next."
#
# The shape of each warning is pinned by unit tests
# (`every_pipeline_warning_names_a_consequence_and_an_action`,
# `the_sample_resolution_warning_names_the_size_and_the_way_out`), but a test can
# only assert about warnings someone remembered to list. This hook closes that
# gap from the other side: a `warn()` call must pass a *named* warning — a
# `*_WARNING` const or a `*_warning()` function — never an inline string. The
# named ones are what the tests enumerate, so a new warning cannot reach a user
# without passing through them.
#
# Only non-test code is scanned. Everything from the first `#[cfg(test)]` in a
# file is a test module, and tests are free to warn with whatever they like.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

status=0

offenders=$(
    python3 - <<'PY'
import re
import sys
from pathlib import Path

# `.warn(` is a call; `fn warn(` is the trait method's definition.
CALL = re.compile(r"\.warn\(\s*(?P<arg>[^\n]*?)\s*\)\s*;")
NAMED = re.compile(r"^&?(?:[A-Z][A-Z0-9_]*_WARNING|[a-z_]+_warning\()")

offenders = []
for source in sorted(Path("crates").glob("*/src/**/*.rs")):
    text = source.read_text()
    # Tests live at the bottom, behind `#[cfg(test)]`.
    body = text.split("#[cfg(test)]", 1)[0]

    for line_number, line in enumerate(body.splitlines(), start=1):
        match = CALL.search(line)
        if match is None:
            continue
        argument = match.group("arg")
        if not NAMED.match(argument):
            offenders.append(f"{source}:{line_number}: warn({argument})")

print("\n".join(offenders))
PY
)

if [[ -n "${offenders}" ]]; then
    echo "error: every warning must be a named, tested string (AGENTS.md, CLI):" >&2
    echo "${offenders}" >&2
    echo "hint: name it \`<thing>_warning()\` or \`<THING>_WARNING\`, say what happened," >&2
    echo "      what it costs, and what to do next, and add it to the test that" >&2
    echo "      enumerates warnings." >&2
    status=1
fi

# The named warnings themselves must carry both halves: an em dash separating
# consequence from action, and a backticked flag or config key to act on.
unactionable=$(
    python3 - <<'PY'
import re
import sys
from pathlib import Path

# A `*_WARNING` const or a `*_warning` fn, from its name to the end of the item.
#
# The const ends at `";` — the closing quote, not the first `;`, because a
# warning's own prose contains semicolons ("...expect ~8 fps; pass `--adapter
# software`..."), and stopping at one would cut the sentence in half exactly
# where the action begins. The fn ends at a `}` in the first column, which is
# where a top-level item ends. Both are non-greedy, so two warnings in a row do
# not merge into one.
ITEM = re.compile(
    r'(?:const\s+(?P<const>[A-Z][A-Z0-9_]*_WARNING)\s*:\s*&str\s*=(?P<const_body>.*?)"\s*;'
    r"|fn\s+(?P<fn>[a-z_]+_warning)\s*\((?P<fn_body>.*?)\n\})",
    re.DOTALL,
)

offenders = []
for source in sorted(Path("crates").glob("*/src/**/*.rs")):
    body = source.read_text().split("#[cfg(test)]", 1)[0]

    for match in ITEM.finditer(body):
        name = match.group("const") or match.group("fn")
        text = match.group("const_body") or match.group("fn_body")

        # The em dash separates what happened from what to do about it.
        if "—" not in text:
            offenders.append(f"{source}: {name} states no consequence (no em dash)")
            continue

        # The action half must quote the flag or config key that answers it. The
        # check is on the half *after* the dash: warnings routinely backtick the
        # path they are about, and a quoted path is not something to do.
        action = text.split("—", 1)[1]
        if "`" not in action:
            offenders.append(f"{source}: {name} names no action (no backticked flag or key)")

print("\n".join(offenders))
PY
)

if [[ -n "${unactionable}" ]]; then
    echo "error: a warning must say what happened and what to do (AGENTS.md, CLI):" >&2
    echo "${unactionable}" >&2
    echo "hint: the canonical shape is" >&2
    echo "      'no GPU adapter found, falling back to software rendering — expect" >&2
    echo "       ~8 fps; pass \`--adapter software\` to silence this'" >&2
    status=1
fi

if [[ ${status} -eq 0 ]]; then
    echo "Every warning is a named string that says what happened and what to do."
fi

exit "${status}"
