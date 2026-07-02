#!/usr/bin/env bash
#
# Harvest the "universe" of Unreal trace events from an engine source tree, for use
# with `uasset utrace coverage --universe <file>`.
#
# It scans for UE_TRACE_EVENT_BEGIN / UE_TRACE_EVENT_BEGIN_EXTERN declarations and
# emits one "Logger.Event" per line, sorted and de-duplicated.
#
# Usage:
#   scripts/harvest-ue-trace-events.sh <Engine/Source dir> [output file]
#
# Example:
#   scripts/harvest-ue-trace-events.sh /path/to/UE/Engine/Source ue_events.txt
#
# Requirements: ripgrep (rg).
#
# Caveats / known gaps (the universe is best-effort, not authoritative):
#   - `$Trace` events use a `$` logger name that this regex does not match; they are
#     declared through a different mechanism.
#   - Some events are declared with macros or across lines this simple regex misses,
#     so `coverage`'s `not_in_universe` list can include real engine events. Improve
#     the pattern here if you need tighter completeness.
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <Engine/Source dir> [output file]" >&2
  exit 64
fi

source_dir="$1"
output="${2:-ue_events.txt}"

if [[ ! -d "$source_dir" ]]; then
  echo "not a directory: $source_dir" >&2
  exit 64
fi

rg -oNI --no-heading \
  'UE_TRACE_EVENT_BEGIN(_EXTERN)?\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*,\s*([A-Za-z_][A-Za-z0-9_]*)' \
  -r '$2.$3' \
  -g '*.h' -g '*.cpp' -g '*.inl' \
  "$source_dir" \
  | sort -u > "$output"

echo "wrote $(wc -l < "$output") distinct trace events to $output" >&2
