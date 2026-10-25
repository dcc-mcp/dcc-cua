import type { NativeRequest, Pairing } from "./protocol.ts";

export type NavigationRequest = Extract<NativeRequest, { method: "navigate" }>;

export interface NavigationOperations {
  isCurrent(): boolean;
  now(): number;
  requirePairing(): Promise<void>;
  foreground(): Promise<boolean>;
  invalidateDocument(): Promise<void>;
  revokePairing(): Promise<void>;
  updateTab(tabId: number, url: string): Promise<void>;
}

function refused(code: string): Error {
  return Object.assign(new Error(code), { code });
}

/** One admitted attempt; cancellation cannot undo a browser navigation. */
export async function navigatePairedTab(
  request: NavigationRequest,
  pairing: Pairing,
  operations: NavigationOperations,
) {
  function beforeDispatch() {
    if (!operations.isCurrent()) throw refused("navigation_cancelled");
    if (operations.now() >= request.deadline_unix_ms) throw refused("command_expired");
  }
  beforeDispatch();
  const url = new URL(request.url);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password
    || url.origin !== pairing.origin || request.tab_id !== pairing.tab_id
    || request.session_nonce !== pairing.session_nonce) {
    throw refused("navigation_target_refused");
  }
  await operations.requirePairing();
  if (!await operations.foreground()) throw refused("target_not_foreground");
  beforeDispatch();
  // Revoke the document even for same-document/fragment navigation or a failed
  // browser API call. No stale ref may survive an admitted navigation attempt.
  await operations.invalidateDocument();
  beforeDispatch();
  await operations.revokePairing();
  beforeDispatch();
  try {
    await operations.updateTab(pairing.tab_id, url.href);
  } catch {
    throw refused("navigation_outcome_unknown");
  }
  if (!operations.isCurrent() || operations.now() >= request.deadline_unix_ms) {
    throw refused("navigation_outcome_unknown");
  }
  return {
    phase: "dispatched",
    navigation_requested: true,
    completion_known: false,
    effect: "unverified",
    pairing_required: true,
    fresh_observation_required: true,
  } as const;
}
