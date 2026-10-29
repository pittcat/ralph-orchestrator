#!/usr/bin/env bash
# Fake `ralph` binary for runner tests.
# Modes via FAKE_RALPH_MODE env:
#   warn-json  — tracing-style WARN line, then pretty JSON, exit 0
#   fail-json  — pretty JSON report, then exit 1 (preset check FAIL)
#   nonzero    — plain text stderr, exit 3
#   bad        — non-JSON stdout, exit 0
#   hang       — sleep past the timeout
case "${FAKE_RALPH_MODE:-warn-json}" in
  warn-json)
    echo '2026-01-01T00:00:00Z  WARN ralph::preflight: Hats source contains core/runtime keys; ignoring them'
    printf '{\n  "passed": true,\n  "warnings": 0,\n  "errors": 0,\n  "findings": []\n}\n'
    ;;
  fail-json)
    printf '{\n  "passed": false,\n  "warnings": 0,\n  "errors": 1,\n  "findings": [{"id":"x","source":"topology","severity":"error","stage":"authoring","message":"boom"}]\n}\n'
    exit 1
    ;;
  nonzero)
    echo 'something went wrong' >&2
    exit 3
    ;;
  bad)
    echo 'this is not json at all'
    ;;
  hang)
    sleep 30
    ;;
esac
