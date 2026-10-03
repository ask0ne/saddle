#!/usr/bin/env bash
# Merge Saddle's hooks into ~/.claude/settings.json.
#
# Everything here MERGES: your existing hooks, statusLine, plugins and
# permissions are never replaced or reordered, and a backup is written before
# any change. Saddle's own entries carry a "# devpit" marker (the app's former
# name, kept so hooks installed by earlier builds are recognised and updated
# in place instead of duplicated).
set -euo pipefail

SETTINGS="${DEVPIT_SETTINGS:-$HOME/.claude/settings.json}"
BACKUP="$SETTINGS.devpit.bak"
EVENTS="$HOME/.devpit/events.jsonl"
MARKER="# devpit"

MODE="install"
DROP_LEGACY=0
for arg in "$@"; do
  case "$arg" in
    --dry-run)          MODE="dry-run" ;;
    --uninstall)        MODE="uninstall" ;;
    --drop-legacy-focus) DROP_LEGACY=1 ;;
    -h|--help)
      echo "usage: $0 [--dry-run] [--uninstall] [--drop-legacy-focus]"
      exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

# Edit the real file if settings.json is a symlink (dotfiles setups) — the
# atomic rename below would otherwise replace the link with a regular file.
if [ -L "$SETTINGS" ]; then
  SETTINGS="$(readlink -f "$SETTINGS")"
  BACKUP="$SETTINGS.devpit.bak"
fi

command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }
if [ ! -f "$SETTINGS" ]; then
  # A fresh Claude Code install may not have written a settings file yet.
  [ "$MODE" = "install" ] || { echo "no settings file at $SETTINGS" >&2; exit 1; }
  mkdir -p "$(dirname "$SETTINGS")"
  echo '{}' > "$SETTINGS"
fi
jq -e . "$SETTINGS" >/dev/null || { echo "$SETTINGS is not valid JSON" >&2; exit 1; }

mkdir -p "$(dirname "$EVENTS")"
touch "$EVENTS"
chmod 600 "$EVENTS"

# One line per firing. -m/timeouts are irrelevant here: it is a local append, so
# it cannot block, and `|| true` means a broken devpit can never fail a hook.
hookcmd() {
  printf 'jq -c --arg e %s '"'"'{e:$e,t:now,d:.}'"'"' >> "$HOME/.devpit/events.jsonl" 2>/dev/null || true  %s' "$1" "$MARKER"
}

JQ_PROG='
def mine: (.command // "") | contains($marker);
def strip_devpit:
  (. // [])
  | map(.hooks = ((.hooks // []) | map(select(mine | not))))
  | map(select((.hooks | length) > 0));
def strip_legacy:
  if $drop_legacy == 1 then
    (. // [])
    | map(.hooks = ((.hooks // []) | map(select((.command // "") | contains("first application process whose name is \"Terminal\"") | not))))
    | map(select((.hooks | length) > 0))
  else . end;
def add($cmd):
  strip_devpit | strip_legacy | . + [{hooks: [{type: "command", command: $cmd}]}];

.hooks = (.hooks // {})
| .hooks.Notification      = ((.hooks.Notification      // []) | add($notif))
| .hooks.Stop              = ((.hooks.Stop              // []) | add($stop))
| .hooks.UserPromptSubmit  = ((.hooks.UserPromptSubmit  // []) | add($prompt))
'

JQ_UNINSTALL='
def mine: (.command // "") | contains($marker);
def strip_devpit:
  (. // [])
  | map(.hooks = ((.hooks // []) | map(select(mine | not))))
  | map(select((.hooks | length) > 0));
.hooks = (.hooks // {})
| .hooks.Notification     = ((.hooks.Notification     // []) | strip_devpit)
| .hooks.Stop             = ((.hooks.Stop             // []) | strip_devpit)
| .hooks.UserPromptSubmit = ((.hooks.UserPromptSubmit // []) | strip_devpit)
| .hooks |= with_entries(select((.value | length) > 0))
'

TMP="$(mktemp -t saddle-settings)"
# The final write target lives in the SAME directory as $SETTINGS, not the
# system tmpdir — mv/rename(2) is only atomic within one filesystem, and
# $TMPDIR is frequently a different one (e.g. tmpfs vs the home volume).
FINAL="$(mktemp "$(dirname "$SETTINGS")/.saddle-settings.XXXXXX")"
trap 'rm -f "$TMP" "$FINAL"' EXIT

if [ "$MODE" = "uninstall" ]; then
  jq --arg marker "$MARKER" "$JQ_UNINSTALL" "$SETTINGS" > "$TMP"
else
  jq --arg marker "$MARKER" \
     --argjson drop_legacy "$DROP_LEGACY" \
     --arg notif  "$(hookcmd Notification)" \
     --arg stop   "$(hookcmd Stop)" \
     --arg prompt "$(hookcmd UserPromptSubmit)" \
     "$JQ_PROG" "$SETTINGS" > "$TMP"
fi

jq -e . "$TMP" >/dev/null || { echo "refusing to write invalid JSON" >&2; exit 1; }

if diff -q <(jq -S . "$SETTINGS") <(jq -S . "$TMP") >/dev/null; then
  echo "no change needed — already up to date"
  exit 0
fi

echo "--- $SETTINGS"
echo "+++ proposed"
diff -u <(jq -S . "$SETTINGS") <(jq -S . "$TMP") || true

if [ "$MODE" = "dry-run" ]; then
  echo
  echo "(dry run — nothing written)"
  exit 0
fi

cp "$SETTINGS" "$BACKUP"
# Preserve the original indentation style (Claude Code writes 2-space).
# Write fully, then rename into place — `jq ... > "$SETTINGS"` directly would
# truncate the real file before writing it, leaving a window where a crash
# (or another process reading settings.json mid-write) sees a corrupt/partial
# file. rename(2) within one filesystem is atomic: readers see either the old
# file or the new one, never a partial write.
jq --indent 2 . "$TMP" > "$FINAL"
# mktemp creates $FINAL as 0600; match $SETTINGS's real mode first, or the
# rename below silently tightens permissions on a file this script doesn't own.
chmod "$(stat -f '%OLp' "$SETTINGS")" "$FINAL"
mv -f "$FINAL" "$SETTINGS"
echo
echo "written. backup at $BACKUP"
if [ "$MODE" = "install" ]; then echo "events -> $EVENTS"; fi
