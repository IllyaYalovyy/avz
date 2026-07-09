#!/usr/bin/env bash
#
# Guards the rendering invariant from AGENTS.md: "One code path: wgpu → Vulkan →
# (hardware driver | lavapipe). Do not add a second renderer or low-fi shader
# variants."
#
# Two ways that breaks. A second wgpu backend compiled in (dx12, metal, gles)
# means shaders can be exercised on a path nobody tests, and golden frames stop
# meaning anything. A second Backends:: flag in the source means the instance
# might enumerate one at runtime even when it was not asked for.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

status=0

# 1. Only the vulkan backend feature is enabled on the wgpu dependency.
for backend in dx12 metal gles webgpu angle webgl; do
    if grep -qE "^wgpu = .*\"${backend}\"" Cargo.toml; then
        echo "error: the wgpu '${backend}' backend is enabled in Cargo.toml (AGENTS.md, rendering)." >&2
        echo "hint: avz renders through Vulkan only; lavapipe covers GPU-less hosts." >&2
        status=1
    fi
done

if ! grep -qE '^wgpu = .*"vulkan"' Cargo.toml; then
    echo "error: the wgpu 'vulkan' backend is not enabled in Cargo.toml." >&2
    echo "hint: Vulkan is avz's only render code path." >&2
    status=1
fi

# 2. No source names a backend other than Vulkan. Comment lines are excluded so
#    the rule can be documented in the code it constrains.
offenders=$(
    grep -rnE 'wgpu::Backends::[A-Za-z_]+' crates/*/src |
        grep -vE 'wgpu::Backends::VULKAN' |
        grep -vE ':[[:space:]]*//' || true
)

if [[ -n ${offenders} ]]; then
    echo "error: a wgpu backend other than VULKAN is named in the source (AGENTS.md, rendering):" >&2
    echo "${offenders}" >&2
    status=1
fi

if [[ ${status} -eq 0 ]]; then
    echo "Rendering has one code path: wgpu → Vulkan → (hardware driver | lavapipe)."
fi

exit "${status}"
