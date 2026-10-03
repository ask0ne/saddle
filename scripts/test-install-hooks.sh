#!/usr/bin/env bash
# Behavioural tests for install-hooks.sh. Runs entirely in a temp HOME; never
# touches the real ~/.claude or ~/.devpit.   usage: scripts/test-install-hooks.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/install-hooks.sh"
command -v jq >/dev/null || { echo "jq is required"; exit 1; }

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); echo "  ok   $1"; }
bad()  { FAIL=$((FAIL+1)); echo "  FAIL $1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

new_env() {
  T="$(mktemp -d)"; export HOME="$T/home"; mkdir -p "$HOME/.claude"
  export DEVPIT_SETTINGS="$HOME/.claude/settings.json"
}
canon() { jq -S . "$1"; }

REAL_HOME="$HOME"
trap 'export HOME="$REAL_HOME"; rm -rf "${T:-}"' EXIT

ORIGINAL='{
  "model": "opus",
  "permissions": {"allow": ["Bash(ls:*)"]},
  "statusLine": {"type": "command", "command": "echo hi"},
  "hooks": {
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard.sh"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]
  }
}'

echo "install into existing settings"
new_env; echo "$ORIGINAL" > "$DEVPIT_SETTINGS"; chmod 644 "$DEVPIT_SETTINGS"
"$SCRIPT" >/dev/null 2>&1
check "exit 0 and valid JSON"            'jq -e . "$DEVPIT_SETTINGS" >/dev/null'
check "adds Notification hook"           'jq -e ".hooks.Notification|length==1" "$DEVPIT_SETTINGS" >/dev/null'
check "adds UserPromptSubmit hook"       'jq -e ".hooks.UserPromptSubmit|length==1" "$DEVPIT_SETTINGS" >/dev/null'
check "Stop keeps the user hook + ours"  'jq -e "[.hooks.Stop[].hooks[].command]|length==2 and (.[0]==\"notify-send done\")" "$DEVPIT_SETTINGS" >/dev/null'
check "PreToolUse untouched"             '[ "$(jq -S .hooks.PreToolUse "$DEVPIT_SETTINGS")" = "$(echo "$ORIGINAL" | jq -S .hooks.PreToolUse)" ]'
check "non-hook keys untouched"          '[ "$(jq -S "del(.hooks)" "$DEVPIT_SETTINGS")" = "$(echo "$ORIGINAL" | jq -S "del(.hooks)")" ]'
check "backup written"                   '[ -f "$DEVPIT_SETTINGS.devpit.bak" ]'
check "backup equals original"           '[ "$(canon "$DEVPIT_SETTINGS.devpit.bak")" = "$(echo "$ORIGINAL" | jq -S .)" ]'
check "file mode preserved (644)"        '[ "$(stat -f %Lp "$DEVPIT_SETTINGS")" = "644" ]'
check "events file is 0600"              '[ "$(stat -f %Lp "$HOME/.devpit/events.jsonl")" = "600" ]'
check "no temp files left behind"        '[ -z "$(ls -A "$HOME/.claude" | grep -E "^\.saddle-settings|tmp" || true)" ]'

echo "idempotence"
BEFORE="$(canon "$DEVPIT_SETTINGS")"
OUT="$("$SCRIPT" 2>&1)"
check "second run reports no change"     'echo "$OUT" | grep -q "no change needed"'
check "second run changes nothing"       '[ "$(canon "$DEVPIT_SETTINGS")" = "$BEFORE" ]'

echo "installed hook command really logs events"
CMD="$(jq -r '.hooks.Notification[-1].hooks[0].command' "$DEVPIT_SETTINGS")"
echo '{"session_id":"abc","notification_type":"permission_prompt","message":"needs you"}' | bash -c "$CMD"
LINE="$(tail -1 "$HOME/.devpit/events.jsonl")"
check "event line has name + payload"    'echo "$LINE" | jq -e ".e==\"Notification\" and .d.session_id==\"abc\" and (.t|type==\"number\")" >/dev/null'
check "hook never fails on bad input"    'echo "not json" | bash -c "$CMD"'

echo "dry run"
new_env; echo "$ORIGINAL" > "$DEVPIT_SETTINGS"; SUM="$(shasum "$DEVPIT_SETTINGS")"
"$SCRIPT" --dry-run >/dev/null 2>&1
check "dry run writes nothing"           '[ "$(shasum "$DEVPIT_SETTINGS")" = "$SUM" ] && [ ! -f "$DEVPIT_SETTINGS.devpit.bak" ]'

echo "uninstall"
new_env; echo "$ORIGINAL" > "$DEVPIT_SETTINGS"
"$SCRIPT" >/dev/null 2>&1; "$SCRIPT" --uninstall >/dev/null 2>&1; RC=$?
check "uninstall exits 0"                '[ $RC -eq 0 ]'
check "uninstall restores original"      '[ "$(canon "$DEVPIT_SETTINGS")" = "$(echo "$ORIGINAL" | jq -S .)" ]'

echo "fresh machine (no settings file)"
new_env
"$SCRIPT" >/dev/null 2>&1
check "creates settings with 3 hooks"    'jq -e "(.hooks|keys)==[\"Notification\",\"Stop\",\"UserPromptSubmit\"]" "$DEVPIT_SETTINGS" >/dev/null'
new_env
"$SCRIPT" --uninstall >/dev/null 2>&1; RC=$?
check "uninstall with no file fails clean" '[ $RC -ne 0 ] && [ ! -f "$DEVPIT_SETTINGS" ]'

echo "symlinked settings.json (dotfiles)"
new_env; mkdir -p "$T/dotfiles"; echo "$ORIGINAL" > "$T/dotfiles/settings.json"; chmod 600 "$T/dotfiles/settings.json"
ln -s "$T/dotfiles/settings.json" "$DEVPIT_SETTINGS"
"$SCRIPT" >/dev/null 2>&1
check "symlink is preserved"             '[ -L "$DEVPIT_SETTINGS" ]'
check "hooks written to the target"      'jq -e ".hooks.Notification|length==1" "$T/dotfiles/settings.json" >/dev/null'
check "target mode preserved (600)"      '[ "$(stat -f %Lp "$T/dotfiles/settings.json")" = "600" ]'

echo "bad input"
new_env; echo '{ not json' > "$DEVPIT_SETTINGS"; SUM="$(shasum "$DEVPIT_SETTINGS")"
"$SCRIPT" >/dev/null 2>&1; RC=$?
check "invalid JSON refused, file intact" '[ $RC -ne 0 ] && [ "$(shasum "$DEVPIT_SETTINGS")" = "$SUM" ]'
"$SCRIPT" --bogus >/dev/null 2>&1; RC=$?
check "unknown flag exits 2"              '[ $RC -eq 2 ]'

echo "legacy hooks from earlier builds are updated in place, not duplicated"
new_env
echo '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"old cmd  # devpit"}]}]}}' > "$DEVPIT_SETTINGS"
"$SCRIPT" >/dev/null 2>&1
check "exactly one Saddle Stop hook"      '[ "$(jq "[.hooks.Stop[].hooks[]|select(.command|contains(\"# devpit\"))]|length" "$DEVPIT_SETTINGS")" = "1" ]'
check "old command replaced"              '! grep -q "old cmd" "$DEVPIT_SETTINGS"'

echo; echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
