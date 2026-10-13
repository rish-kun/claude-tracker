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

# Missing binary: gate events block, others pass.
for ev in user-prompt-submit pre-tool-use; do
  HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook "$ev" </dev/null >/dev/null 2>&1
  check "missing binary blocks $ev" 2 $?
done
HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook session-start </dev/null >/dev/null 2>&1
check "missing binary does not fail session-start" 0 $?

# Success passes stdout through.
fake ok 0 '{"decision":"block"}'
out=$(CCTRACK_BIN="$tmp/ok" sh "$run" hook user-prompt-submit </dev/null)
check "exit 0 passes" 0 $?
[ "$out" = '{"decision":"block"}' ] && echo "ok   - stdout passthrough" || { echo "FAIL - stdout passthrough: $out"; fails=$((fails + 1)); }

# Crash (e.g. panic = 101) blocks gate events, not others.
fake crash 101 ''
CCTRACK_BIN="$tmp/crash" sh "$run" hook pre-tool-use </dev/null >/dev/null 2>&1
check "crash blocks pre-tool-use" 2 $?
CCTRACK_BIN="$tmp/crash" sh "$run" hook stop </dev/null >/dev/null 2>&1
check "crash does not fail stop" 0 $?

# Explicit exit 2 stays 2.
fake two 2 ''
CCTRACK_BIN="$tmp/two" sh "$run" hook user-prompt-submit </dev/null >/dev/null 2>&1
check "exit 2 stays 2" 2 $?

[ "$fails" -eq 0 ] || { echo "$fails failure(s)"; exit 1; }
echo "all wrapper tests passed"
