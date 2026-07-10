#!/usr/bin/env bash
#
# Guards the two decisions that keep the looped background video from becoming
# the failure VISION.md §11 predicts for it: "bg video decode thread
# deadlocks/stalls pipeline ... Mitigation: bounded channel + timeout with clear
# error".
#
#   1. The frame queue is bounded. Every decoder outruns a software render, so an
#      unbounded channel reads the whole loop into memory while the renderer
#      falls behind - a slow leak that grows with the song, on a machine that was
#      already chosen for having no GPU.
#   2. The render thread waits for a frame with a timeout. A decoder that wedges
#      otherwise wedges the render, silently, for as long as anyone leaves it.
#
# Neither is visible to a test that renders successfully: an unbounded channel
# delivers exactly the same frames, and a blocking `recv()` returns them just as
# fast. Only the source says which one is there.
#
# The stall path itself is covered by
# `a_decoder_that_stops_producing_frames_times_out_and_names_the_video`; this
# hook is what keeps a later refactor from deleting the bound it asserts nothing
# about.

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "${repo_root}"

reader=crates/avz-core/src/render/video.rs

if [[ ! -f ${reader} ]]; then
    echo "error: ${reader} is missing: the background video reader lives there." >&2
    exit 1
fi

# Only the production half of the file is scanned. `mod tests` asserts on the
# very strings the checks below grep for -- `arg == "-an"` is one -- so a hook
# that read the whole file would stay green on a reader that had lost the flag,
# kept alive by the test that was supposed to catch it.
#
# Comment lines are excluded so the rule can be documented in the code it
# constrains, exactly as in 10-core-is-ui-agnostic.sh.
code=$(sed '/^#\[cfg(test)\]/,$d' "${reader}" | grep -vE '^[[:space:]]*(//|\*)' || true)

status=0

if ! grep -q 'sync_channel' <<<"${code}"; then
    echo "error: ${reader} must feed frames through a bounded channel (VISION.md §11)." >&2
    echo "hint: sync_channel(FRAME_QUEUE), never channel(): a decoder outruns every render." >&2
    status=1
fi

if grep -qE '(^|[^_[:alnum:]])channel\(\)' <<<"${code}"; then
    echo "error: ${reader} uses an unbounded channel; the whole loop would buffer in memory." >&2
    status=1
fi

if ! grep -q 'recv_timeout' <<<"${code}"; then
    echo "error: ${reader} must wait for a frame with a timeout (VISION.md §11)." >&2
    echo "hint: a stalled decoder is an error naming the video, never a render that hangs." >&2
    status=1
fi

if grep -qE '\.recv\(\)' <<<"${code}"; then
    echo "error: ${reader} blocks forever on recv(); a wedged decoder must time out." >&2
    status=1
fi

# The background video is muted by construction (VISION.md §5.3): its audio
# stream is never decoded, so it can never reach the mux.
if ! grep -q '"-an"' <<<"${code}"; then
    echo "error: ${reader} must pass -an: the background video's audio is never decoded." >&2
    status=1
fi

if [[ ${status} -eq 0 ]]; then
    echo "the background video reader is bounded, times out, and decodes no audio."
fi

exit "${status}"
