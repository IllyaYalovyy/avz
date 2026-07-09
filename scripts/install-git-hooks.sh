#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "${repo_root}"

if [[ ! -d .git ]]; then
    printf 'No .git directory found; skipping local git hook installation.\n' >&2
    exit 0
fi

mkdir -p .git/hooks

existing_hook=.git/hooks/pre-commit
chained_hook=.git/hooks/pre-commit.local

if [[ -f "${existing_hook}" ]] && ! grep -q 'AI_FILE_GUARD_BEGIN' "${existing_hook}"; then
    cp "${existing_hook}" "${chained_hook}"
    chmod +x "${chained_hook}"
fi

cat > "${existing_hook}" <<'HOOK'
#!/usr/bin/env bash
set -euo pipefail

# AI_FILE_GUARD_BEGIN
blocked=$(
  git diff --cached --name-only --diff-filter=ACMR |
    grep -E '(^|/)(\.ktask|\.codex|\.claude|\.claude_[^/]*|\.kiro|\.aim|\.cursor|\.copilot|\.continue|\.windsurf|\.chat_history|\.ai|\.agent)(/|$)|(^|/)(\.aider[^/]*|.*\.prompt|.*\.prompt\.md|.*\.context|.*\.context\.md)(/|$)' || true
)

if [[ -n "${blocked}" ]]; then
  echo "Refusing to commit AI/task-runner files, prompts, context, or chat logs:" >&2
  echo "${blocked}" >&2
  echo >&2
  echo "Remove them from the index with: git restore --staged <file>" >&2
  exit 1
fi
# AI_FILE_GUARD_END

if [[ -x .git/hooks/pre-commit.local ]]; then
  .git/hooks/pre-commit.local "$@"
fi
HOOK

chmod +x "${existing_hook}"
printf 'Installed local AI-file pre-commit guard at %s.\n' "${existing_hook}"
