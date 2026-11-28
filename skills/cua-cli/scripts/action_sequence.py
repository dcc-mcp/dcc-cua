"""Run a finite action plan on the caller's existing, attested DCC-CUA task.

call(method, params) must return the task's checked structured receipt and raise
on MCP or Host failure. journal is caller-owned and survives exceptions. verify,
when supplied, checks the application's effect before the next step.
"""

from time import perf_counter


def run_sequence(steps, dispatch, journal, verify=None):
    if not isinstance(steps, list) or not 1 <= len(steps) <= 64:
        raise ValueError("expected 1–64 declared steps")
    for number, step in enumerate(steps, 1):
        if not isinstance(step, dict) or not isinstance(step.get("action"), str):
            raise ValueError(f"invalid step {number}")
    for number, step in enumerate(steps, 1):
        entry = {"step": number, "action": step["action"], "status": "pending"}
        journal.append(entry)
        start = perf_counter()
        try:
            receipt = dispatch(step, number == len(steps))
            entry["dispatch_ms"] = round((perf_counter() - start) * 1000)
            entry["status"] = "delivered"
            if verify is not None:
                entry["readback"] = verify(step, receipt)
                entry["status"] = "verified"
        except Exception:
            entry["status"] = "verification_failed" if entry["status"] == "delivered" else "dispatch_unconfirmed"
            raise
        finally:
            entry["elapsed_ms"] = round((perf_counter() - start) * 1000)


def window_sequence(call, target, initial_snapshot, steps, journal, verify=None):
    """Use ordinary execute_action payloads and reuse each post-action capture."""
    def bound_id(snapshot):
        observed = snapshot["observation"]
        if (observed["process_id"], observed["window_handle"]) != target:
            raise RuntimeError("sequence observation target changed")
        return snapshot["observation_id"]

    observation_id = bound_id(initial_snapshot)

    def dispatch(action, last):
        nonlocal observation_id
        receipt = call("execute_action", {"observation_id": observation_id, "action": action,
                                         "capture_after": not last})
        if receipt.get("status") != "succeeded":
            raise RuntimeError("window action did not complete")
        if not last:
            post = receipt["post_snapshot"]
            if post.get("success") is False:
                if post.get("action_was_executed") is not True or receipt.get("observation_required") is not True:
                    raise RuntimeError("post-action capture is ambiguous")
                post = call("snapshot", {})
            observation_id = bound_id(post)
        return receipt

    run_sequence(steps, dispatch, journal, verify)


def browser_pointer_sequence(call, target_id, tab_id, steps, journal, verify=None):
    """Resolve unique labels from a fresh semantic snapshot on the bound tab."""
    target = {"target_id": target_id, "tab_id": tab_id}
    for step in steps:
        if step.get("action") not in {"move", "click", "drag"} or not step.get("target"):
            raise ValueError("expected a named move, click or drag target")
        if (step["action"] == "drag") != bool(step.get("destination")):
            raise ValueError("only a drag takes a destination")

    def dispatch(step, _last):
        seen = call("browser_snapshot", dict(target, snapshot_format="semantic_v2"))

        def ref(label):
            matches = [row["ref"] for row in seen["refs"] if row.get("label", row.get("name")) == label]
            if len(matches) != 1:
                raise RuntimeError(f"expected one current browser reference for {label!r}")
            return matches[0]

        request = dict(target, snapshot_id=seen.get("snapshot_id") or seen["snapshot"]["id"],
                       ref=ref(step["target"]), input_route="trusted")
        if step["action"] == "click":
            return call("browser_click", request)
        request["action"] = "hover" if step["action"] == "move" else "drag"
        if step["action"] == "drag":
            request["destination_ref"] = ref(step["destination"])
        return call("browser_pointer", request)

    run_sequence(steps, dispatch, journal, verify)
