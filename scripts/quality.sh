#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "${repo_root}"

log() {
    printf '\n==> %s\n' "$*"
}

has_command() {
    command -v "$1" >/dev/null 2>&1
}

json_has_script() {
    local script_name=$1
    python3 - "$script_name" <<'PY'
import json
import sys
from pathlib import Path

script = sys.argv[1]
path = Path("package.json")
if not path.exists():
    raise SystemExit(1)

try:
    data = json.loads(path.read_text())
except json.JSONDecodeError:
    raise SystemExit(1)

raise SystemExit(0 if script in data.get("scripts", {}) else 1)
PY
}

run_shell_syntax_checks() {
    if compgen -G "scripts/*.sh" >/dev/null; then
        log "Checking shell script syntax"
        bash -n scripts/*.sh
    fi
}

run_rust_checks() {
    if [[ ! -f Cargo.toml ]]; then
        return
    fi

    if ! has_command cargo; then
        log "Skipping Rust checks: cargo not found"
        return
    fi

    log "Rust format"
    cargo fmt --check

    log "Rust clippy"
    cargo clippy --all-targets --all-features -- -D warnings

    log "Rust tests"
    cargo test --all-targets --all-features
}

run_node_checks() {
    if [[ ! -f package.json ]]; then
        return
    fi

    if ! has_command npm; then
        log "Skipping Node checks: npm not found"
        return
    fi

    if [[ -f package-lock.json && ! -d node_modules ]]; then
        log "Installing Node dependencies"
        npm ci
    fi

    if json_has_script lint; then
        log "Node lint"
        npm run lint
    fi

    if json_has_script test; then
        log "Node tests"
        npm test
    fi

    if json_has_script build; then
        log "Node build"
        npm run build
    fi
}

run_python_checks() {
    if [[ ! -d tests && ! -d test ]]; then
        return
    fi

    if ! has_command python3; then
        log "Skipping Python checks: python3 not found"
        return
    fi

    if python3 -m pytest --version >/dev/null 2>&1; then
        log "Python tests"
        python3 -m pytest
    else
        log "Skipping Python tests: pytest not found"
    fi
}

run_project_hooks() {
    if [[ ! -d scripts/quality.d ]]; then
        return
    fi

    local hook
    # -u+x, not -111: hooks run as their owner, and a restrictive umask (0027)
    # clears the other-execute bit that -111 demands. Matching on -111 made the
    # gate skip every hook without saying so.
    while IFS= read -r hook; do
        log "Project quality hook: ${hook}"
        "${hook}"
    done < <(find scripts/quality.d -maxdepth 1 -type f -perm -u+x | sort)
}

run_shell_syntax_checks
run_rust_checks
run_node_checks
run_python_checks
run_project_hooks

log "Quality gate passed"
