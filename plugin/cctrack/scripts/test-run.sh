#!/bin/sh
# Tests for scripts/run: gate events must fail closed.
set -u
here=$(cd "$(dirname "$0")" && pwd)
run="$here/run"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fails=0

fake() { # fake <name> <exit-code> <stdout>
  { echo '#!/bin/sh'; printf "printf '%%s' '%s'\n" "$3"; echo "exit $2"; } > "$tmp/$1"
  chmod +x "$tmp/$1"
}

check() { # check <description> <expected-exit> <actual-exit>
  if [ "$2" -eq "$3" ]; then echo "ok   - $1"; else echo "FAIL - $1 (expected $2, got $3)"; fails=$((fails + 1)); fi
}

has() { # has <description> <haystack> <needle>...
  desc=$1; hay=$2; shift 2
  for needle in "$@"; do
    case "$hay" in
      *"$needle"*) ;;
      *) echo "FAIL - $desc: missing [$needle] in [$hay]"; fails=$((fails + 1)); return ;;
    esac
  done
  echo "ok   - $desc"
}

lacks() { # lacks <description> <haystack> <needle>
  case "$2" in
    *"$3"*) echo "FAIL - $1: unexpected [$3] in [$2]"; fails=$((fails + 1)) ;;
    *) echo "ok   - $1" ;;
  esac
}

block_json='{"decision":"block","reason":"'
deny_json='{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"'

# Missing binary: gate events block, others pass.
for ev in user-prompt-submit pre-tool-use; do
  HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook "$ev" </dev/null >/dev/null 2>&1
  check "missing binary blocks $ev" 2 $?
done
HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook session-start </dev/null >/dev/null 2>&1
check "missing binary does not fail session-start" 0 $?

# Missing binary: gate events also print the hook's own block/deny JSON, and
# still explain on stderr.
out=$(HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook user-prompt-submit </dev/null 2>"$tmp/err")
has "missing binary prints block JSON for user-prompt-submit" "$out" "$block_json" "CCTRACK_BIN" '"}'
has "missing binary explains on stderr" "$(cat "$tmp/err")" "not installed" "CCTRACK_BIN"
out=$(HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook pre-tool-use </dev/null 2>/dev/null)
has "missing binary prints deny JSON for pre-tool-use" "$out" "$deny_json" "CCTRACK_BIN" '"}}'
out=$(HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook session-start </dev/null 2>/dev/null)
[ -z "$out" ] && echo "ok   - missing binary prints nothing for session-start" || { echo "FAIL - session-start stdout: $out"; fails=$((fails + 1)); }

# CCTRACK_BIN unset and cctrack nowhere to be found: blocks.
if [ -e /opt/homebrew/bin/cctrack ] || [ -e /usr/local/bin/cctrack ]; then
  echo "skip - cctrack installed system-wide; cannot test 'not found anywhere'"
else
  mkdir -p "$tmp/emptyhome"
  out=$(unset CCTRACK_BIN; HOME="$tmp/emptyhome" PATH=/usr/bin:/bin sh "$run" hook user-prompt-submit </dev/null 2>/dev/null)
  check "unset CCTRACK_BIN with nothing installed blocks" 2 $?
  has "unset CCTRACK_BIN with nothing installed prints block JSON" "$out" "$block_json"
fi

# CCTRACK_BIN unset: found in ~/.local/bin.
mkdir -p "$tmp/home/.local/bin"
{ echo '#!/bin/sh'; echo 'printf found'; } > "$tmp/home/.local/bin/cctrack"
chmod +x "$tmp/home/.local/bin/cctrack"
out=$(unset CCTRACK_BIN; HOME="$tmp/home" PATH=/usr/bin:/bin sh "$run" hook user-prompt-submit </dev/null 2>/dev/null)
check "finds ~/.local/bin/cctrack" 0 $?
[ "$out" = found ] && echo "ok   - ~/.local/bin/cctrack ran" || { echo "FAIL - ~/.local/bin/cctrack output: $out"; fails=$((fails + 1)); }

# Non-executable CCTRACK_BIN blocks.
printf '#!/bin/sh\nexit 0\n' > "$tmp/noexec"
chmod -x "$tmp/noexec"
out=$(CCTRACK_BIN="$tmp/noexec" sh "$run" hook pre-tool-use </dev/null 2>/dev/null)
check "non-executable CCTRACK_BIN blocks pre-tool-use" 2 $?
has "non-executable CCTRACK_BIN prints deny JSON" "$out" "$deny_json"

# Success passes stdout through.
fake ok 0 '{"decision":"block"}'
out=$(CCTRACK_BIN="$tmp/ok" sh "$run" hook user-prompt-submit </dev/null)
check "exit 0 passes" 0 $?
[ "$out" = '{"decision":"block"}' ] && echo "ok   - stdout passthrough" || { echo "FAIL - stdout passthrough: $out"; fails=$((fails + 1)); }

# Hook input on stdin reaches the binary.
{ echo '#!/bin/sh'; echo 'cat'; } > "$tmp/echo"
chmod +x "$tmp/echo"
out=$(printf '%s' '{"tool_name":"Bash"}' | CCTRACK_BIN="$tmp/echo" sh "$run" hook pre-tool-use)
[ "$out" = '{"tool_name":"Bash"}' ] && echo "ok   - stdin reaches the binary" || { echo "FAIL - stdin passthrough: $out"; fails=$((fails + 1)); }

# Crash (e.g. panic = 101) blocks gate events, not others; the child's partial
# stdout is never forwarded, the wrapper's deny JSON is printed instead.
fake crash 101 'garbage'
out=$(CCTRACK_BIN="$tmp/crash" sh "$run" hook pre-tool-use </dev/null 2>"$tmp/err")
check "crash blocks pre-tool-use" 2 $?
has "crash prints deny JSON" "$out" "$deny_json" "exit 101"
lacks "crash does not forward child stdout" "$out" "garbage"
has "crash explains on stderr" "$(cat "$tmp/err")" "exit 101"
out=$(CCTRACK_BIN="$tmp/crash" sh "$run" hook stop </dev/null 2>/dev/null)
check "crash does not fail stop" 0 $?
lacks "crash does not forward child stdout for stop" "$out" "garbage"

# Explicit exit 2 stays 2, with block JSON.
fake two 2 ''
out=$(CCTRACK_BIN="$tmp/two" sh "$run" hook user-prompt-submit </dev/null 2>/dev/null)
check "exit 2 stays 2" 2 $?
has "exit 2 prints block JSON" "$out" "$block_json"

[ "$fails" -eq 0 ] || { echo "$fails failure(s)"; exit 1; }
echo "all wrapper tests passed"
