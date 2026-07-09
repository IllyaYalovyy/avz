#!/usr/bin/env bash
#
# Guards UT-003 (VISION.md §3, §5.3, §7): on a host with no GPU, `--adapter auto`
# falls back to lavapipe and flags the fallback so the CLI can warn, while
# `--adapter gpu` fails fast with a message naming the escape hatch.
#
# Developer machines have a GPU, so the normal test run never walks that path.
# Restricting Vulkan to the lavapipe ICD simulates the GPU-less host, which is
# what `docs/TESTING.md` otherwise marks as "manual: needs a GPU-less host".
#
# The assertions live in the render tests. AVZ_TEST_EXPECT_NO_GPU turns the ones
# that tolerate either adapter into ones that demand the fallback, so this hook
# fails if the ICD restriction ever stops taking effect.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Skipping GPU-less fallback check: cargo not found"
    exit 0
fi

# Mesa installs one ICD manifest per architecture, e.g. lvp_icd.x86_64.json and
# lvp_icd.i686.json. Only the one matching the test binary can be loaded; the
# others fail with "no Vulkan adapter found", which looks like a missing driver.
icd="/usr/share/vulkan/icd.d/lvp_icd.$(uname -m).json"

if [[ ! -f ${icd} ]]; then
    echo "Skipping GPU-less fallback check: no lavapipe ICD at ${icd}."
    echo "hint: install it with 'sudo dnf install mesa-vulkan-drivers' on Fedora."
    exit 0
fi

echo "Simulating a GPU-less host with ${icd}"

# VK_DRIVER_FILES is the current name; VK_ICD_FILENAMES is honored by older
# loaders. Setting both keeps this working across Vulkan loader versions.
AVZ_TEST_EXPECT_NO_GPU=1 \
    VK_DRIVER_FILES="${icd}" \
    VK_ICD_FILENAMES="${icd}" \
    cargo test --quiet --package avz-core --test offscreen_readback

echo "GPU-less host: auto falls back to lavapipe and flags it; gpu fails fast."
