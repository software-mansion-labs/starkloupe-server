#!/usr/bin/env python3
"""Assert the structural facts of one simulation response.

Driven by scripts/e2e-test.sh through the E2E_* environment variables. Kept
separate so the expectations are readable rather than buried in shell quoting.
"""

import json
import os
import sys


def fail(msg):
    print(f"FAIL {os.environ['E2E_NAME']}: {msg}", file=sys.stderr)
    sys.exit(1)


def main():
    name = os.environ["E2E_NAME"]
    expected = os.environ["E2E_EXPECTED"]
    expected_calls = int(os.environ["E2E_CALLS"])
    expected_failed = int(os.environ["E2E_FAILED"])

    with open(sys.argv[1]) as handle:
        payload = json.load(handle)

    l2 = payload.get("l2_transaction_data")
    if not l2:
        fail(f"no l2_transaction_data in response: {json.dumps(payload)[:300]}")

    result = l2["simulation_result"]
    execution = result.get("execution_result") or {}
    status = execution.get("execution_status")

    if ":" in expected:
        want_status, want_reason = expected.split(":", 1)
        if status != want_status:
            fail(f"expected {want_status}, got {status}")
        reason = execution.get("revert_reason")
        if reason != want_reason:
            fail(f"expected revert_reason {want_reason!r}, got {reason!r}")
    elif status != expected:
        fail(f"expected {expected}, got {status}")

    calls = result.get("contract_calls_map") or {}
    if len(calls) != expected_calls:
        fail(f"expected {expected_calls} contract calls, got {len(calls)}")

    failed = [k for k, v in calls.items() if v.get("is_failed")]
    if len(failed) != expected_failed:
        fail(f"expected {expected_failed} failed calls, got {len(failed)}: {sorted(failed)}")

    # Every call must carry a serialised result. A call that failed has to report
    # an Err, and one that succeeded an Ok — this is what catches a regression in
    # the cheatnet trace-result types or their Serialize impls.
    for call_id, call in calls.items():
        if "result" not in call:
            fail(f"call {call_id} has no result field")
        payload_keys = set(call["result"]) if isinstance(call["result"], dict) else set()
        if call.get("is_failed") and "Err" not in payload_keys:
            fail(f"call {call_id} is_failed but result is {json.dumps(call['result'])[:80]}")
        if not call.get("is_failed") and "Ok" not in payload_keys:
            fail(f"call {call_id} succeeded but result is {json.dumps(call['result'])[:80]}")

    # A revert must decode its panic data into readable text rather than leaking
    # raw felts to the caller.
    if expected.startswith("REVERTED"):
        recoverable = [
            c for c in calls.values()
            if isinstance(c.get("result"), dict)
            and "Recoverable" in (c["result"].get("Err") or {})
        ]
        if not recoverable:
            fail("expected at least one Recoverable failure among the calls")

    print(f"  ok {name}: {status}, {len(calls)} calls, {len(failed)} failed")


if __name__ == "__main__":
    main()
