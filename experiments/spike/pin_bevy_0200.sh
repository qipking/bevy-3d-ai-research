#!/usr/bin/env bash
# Pin the whole Bevy tree to 0.20.0.
#
# WHY: bevy_winit 0.20.1 fails to compile:
#   error[E0004]: pattern `&bevy_window::CursorIcon::Custom(_)` not covered
#   --> bevy_winit-0.20.1/src/cursor/mod.rs:124
# bevy_feathers 0.20.1 depends on the bevy 0.20.1 tree, so we use
# bevy_feathers 0.20.0 (which depends on the 0.20.0 tree) instead.
#
# bevy_luma already targets bevy 0.20.0, so this makes the whole graph consistent.
set -uo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

CRATES=$(grep -A2 '^name = "bevy' Cargo.lock \
  | grep -E '^name = ' \
  | sed -E 's/name = "([^"]+)"/\1/' \
  | sort -u)

for c in $CRATES; do
  out=$(cargo update -p "$c" --precise 0.20.0 2>&1)
  if echo "$out" | grep -q "Downgrading"; then
    echo "OK   $c"
  fi
done

echo "--- remaining non-0.20.0 bevy crates ---"
remaining=$(grep -A2 '^name = "bevy' Cargo.lock | grep -E '^name|^version' \
  | paste - - | grep -v '0\.20\.0' | grep -v 'bevy_mikktspace')
if [ -z "$remaining" ]; then
  echo "(none - all pinned to 0.20.0)"
else
  echo "$remaining"
fi
