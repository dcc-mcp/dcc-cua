# The Bazaar profile package

This example separates three authorities:

- `profile.json` supplies deterministic application vocabulary and a bounded loopback state-source contract.
- `SKILL.md` supplies decision policy, including evidence-gated pivots and transaction postconditions.
- `companion/` is an optional, independently built read-only process that tails `Player.log` and indexes `GameData.db`. The profile never launches it and it exposes no action endpoint.
- `knowledge/card-identities.seed.json` supplies reviewed external aliases that users may copy into their own local knowledge cache.

The ten-win corpus is a population prior, not a route to force. Runtime decisions prioritize the current verified board, active and broken relationships, recent combat records, and only then historical builds. Economy and progress values that cannot be derived from the log must be supplied as volatile observations with provenance and the exact `run.runId`; a missing or mismatched run ID fails closed as stale. Do not publish one player's live values in a reusable package.

The companion schema pinned by `profile.json` exposes a geometry-verified `decisionSupport.boardCapacity` contract. `physicalSlots=10` describes only the coordinate space. `unlockedSlotIds` and `unlockedCapacity` define current playable capacity; `occupiedSlots` and `openUnlockedSlots` count only that verified unlocked set. `fitPlacements` lists exact contiguous placements for known stash instances, and `fill_board_from_stash_before_player_combat` is emitted only when at least one such placement exists.

An authoritative current-run `PlayerSnapshotDTO.UnlockedSlots`, `TPlayerInventory.UnlockedSlots`, or `GameSimEventSocketsUnlocked.UnlockedSocketsBitmask` wins over every inferred value. Build `1.0.11894` may fall back to a verified observation bound to the exact `run.runId` and `run.stateTickId`: level 1 unlocks sockets 3-6, level 2 unlocks 2-7, level 3 unlocks 1-8, and level 4 or later unlocks 0-9. A missing mask, unsupported build, unverified observation, or stale state tick fails closed. Final-socket-only movement logs expose `placementReceipts` with `desiredSocket=null` and `clamp=unknown`; they never turn the game's lock clamp or packing into an invented input failure.

Storage geometry is a separate evidence domain from stash identity completeness. `physicalSlots=10` bounds the storage coordinate space, while `usableSlotIds` comes from the current observation. A user-owned `currentStorageObservation` may expose `decisionSupport.storageCapacity` only when it is verified and bound to the exact current `runId`, state tick, and CUA observation. `candidateFitPlacements` then lists contiguous fits for exact dealt item candidates; wrong-run, stale, unverified, fragmented, unknown-size, or out-of-range evidence fails closed. Opening the backpack is only a way to obtain this evidence, not an unconditional purchase macro, and it does not upgrade the incremental `stateCompleteness.stash` field to complete.

`purchaseReceipts` contains bounded, run-scoped positive commits. For a new dragged item, the companion correlates the exact final-move instance, a nearby `SelectItemCommand`, and the exact `Card Purchased` instance/template/target. The command log has no request or instance identifier, so `selectItemCommandSeenInWindow` is only bounded temporal evidence and never an instance-level request claim. A delivered Host action without a fresh receipt proves no observed purchase mutation; it does not diagnose storage visibility, capacity, focus, or input delivery as the cause. `placementReceipts` remains movement-only.

Schema `3.3.0` adds a Loot reward transaction contract. Every current candidate has a `candidateActionableRegions` entry whether its identity is resolved or unresolved. These normalized crops are layout hints and always require a fresh CUA observation fence; the single-candidate `LootState` center crop is explicitly `unverified_layout_hint`. Skills publish `primaryGesture=left_click`, while right-click is not claimed as a selection contract.

`rewardOutcomes` is a bounded, run-scoped ledger that is independent from the mutable next-choice list. A skill claim is finalized only after the exact `Selected skill`, ordered `SelectSkillCommand` send/response, transition out of `LootState`, and a subsequent disposal batch that does not contain the selected instance. A Loot item is finalized only from the exact candidate's `Card Purchased` physical board/storage destination plus the Loot transition. `ExitCurrentStateCommand` followed by the Loot transition and disposal of the exact pending candidate set is published as `discarded`; a later `Cards Dealt` cannot erase an unfinished outcome.

Schema `3.4.0` adds a choice transaction contract without widening the Loot ledger. `choiceFence` exists only when the current choice has an exact `runId`, semantic `stateTickId`, `selectionMessageId`, `choiceKind`, and ordered `candidateInstanceIds`. That complete value is the idempotency key for one decision; a missing component fails closed. An unresolved identity still blocks candidate selection, but an explicit `discard_current_choice` may be offered because it makes no claim about which candidate was purchased or selected.

Schema `3.5.0` adds `decisionSupport.upgradeOpportunity`. `fusionPairs` evaluates only exact same-template pairs with at least one owned instance: equal verified Bronze, Silver, or Gold tiers publish `canFuse=true` and `direct_fusion_candidate`, while unequal verified tiers publish `canFuse=false`, `not_fusible`, and `blockedReason=tier_mismatch`; only Diamond is `maximum_tier`. Missing identity/template/tier/provenance and unsupported tier labels remain `unknown`. Instance IDs are canonicalized before pairing, self-pairs are rejected, and conflicting evidence for one repeated ID produces one blocked unknown guard. Because an unresolved offer could match any known item, unresolved or conflicting candidate evidence propagates to every participating mutation guard; an otherwise known peer cannot fall through to `none`. Exact known `fusionPairs` remain available as positive evidence even when a separate peer forces the safer guard. This pair result never proves that no other material exists. `mutationGuards` therefore keeps `upgradeOpportunity=unknown` whenever board or incremental stash coverage cannot prove absence, and supplies a reason before an owned or offered card is sold or skipped. In a resolved `PedestalState`, only an exact local "Upgrade an Item" identity exposes `pedestalCandidates`; each candidate retains the same instance ID, advances one tier, and carries the current enchantment plus its provenance. The prediction does not replace the post-drag authoritative tier/enchantment commit.

Schema `3.6.0` adds the bounded, run-scoped `inventoryMutationReceipts` ledger for exact sells and two-instance atomic swaps. A receipt owns its immutable `runId`, semantic `stateTickId`, final `logCursor`, source `mutationFence`, exact ordered instance set, before/after physical locations, and `sameFencePolicy=deny_repeat_use_cached_receipt`. The Player log must contain one bounded local mutation batch, one command send, one captured request ID, its response, one owning `Processing [NetMessageGameSim]`, and the same owner's `Finished`; a sell additionally requires the exact in-owner `Sold Card` value. A swap is log-committed only when exactly two distinct instances exchange each other's exact locations as a bijection. Missing locations, single-sided moves, duplicate command/owner events, owner mismatches, early/late sell commits, and time/cursor overflow remain pending forever.

Player-log completion alone publishes `logCommitted=true`, `finalized=false`, and `status=awaiting_verified_observation`. Finalization requires one `currentInventoryMutationObservations` entry whose verified fresh frame is bound to that exact receipt's run, state tick, log cursor, operation, instance set, and non-empty observation ID. Its exact post-locations must match; sells must also match the gold delta and either verify the resolved on-sell tooltip effect or explicitly prove that no resolved effect applies. Stale or conflicting companion observations never fill missing Player-log state, never create another receipt, and never authorize a repeat action. `fixtures/inventory-mutations-authoritative.log` labels its synthetic bootstrap separately from the unchanged observed Player-log transaction suffix.

`decisionOutcomes` records that explicit choice decision separately from `rewardOutcomes`. Each receipt repeats the immutable source owner as `sourceSelectionMessageId`; `commitMessageId` remains null until one exact GameSim batch commits the discard. A source or commit message owns evidence only when its `Processing [NetMessageGameSim]` ID was observed before the relevant deal, transition, and disposal events and the closing `Finished processing [NetMessageGameSim]` carries that same ID. An ownerless `Finished`, a late `Processing`, or a mismatched ID cannot claim already observed evidence. The commit state machine is strict: one `ExitCurrentStateCommand` send, its response, one owning `Processing`, the exact `EncounterState` to `ChoiceState` transition, exactly one next `Cards Dealt`, exact ordered disposal of the old fenced candidates, and the matching owner `Finished`. Any early, duplicate, missing, extra, mismatched, or fenced-candidate-purchase step immediately sets `evidence.batchConflicted=true`; that receipt stays pending and can never recover or finalize from later lines. A new fence cannot replace an active pending fence: its observed Exit is recorded as a finalized `active_discard_fence_conflict` denial while the source fence remains pending.

Every pending, finalized, or denied receipt carries `sameFencePolicy=deny_repeat_use_cached_receipt`; while a receipt for the current fence exists, `discard_current_choice` is withheld and callers reuse the cached receipt. Merchant purchases remain `purchaseReceipts`; they are never relabeled as Loot rewards. In `fixtures/magma-core-event-option-discarded.log`, lines 1-13 are a curated excerpt of events actually observed in `Player.log`; lines 14-20 are an explicitly synthetic future commit chain used only to test the postcondition. Those seven lines are not live evidence that the current game action occurred.

BazaarPlusPlus's Tab collection panel is not treated as an API. Most reusable card identity and mechanics data comes from the local `GameData.db`; current-run state comes from exact Player-log receipts and reviewed observations. Filtering, sorting, and other in-process Tab view-model state remain outside this companion unless a separately versioned, read-only adapter is installed.

`candidateIdentityProvider.kind=bpp_combat_replay` is an optional read-only identity source for the choice message emitted directly by a completed combat replay. It decodes BazaarPlusPlus `PvpReplayPayload` version 1 as typed MessagePack: gzip framing, MessagePack-CSharp LZ4 block-array framing, the pinned positional despawn message, and only the two known card-creation union variants. It never searches arbitrary bytes or strings. A mapping is published only when the replay message ID and ordered choice-instance list exactly match the current `Player.log` message, the snapshot is rebound to the exact current `runId` and `stateTickId`, every choice has one mapping, and every canonical template GUID exists in the current read-only `GameData.db`. Any missing, stale, ambiguous, malformed, unsupported, or unknown-template evidence rejects the whole mapping and is reported through `provenance.candidateIdentity`.

This replay source intentionally does not claim to be a live game-state API. A follow-up choice generated after selecting one of the replay's initial entries is a different network message and is not present in that replay payload; it remains unresolved unless `Player.log` supplies its template or a separately versioned live read-only provider is installed. The provider seam exists so that coverage can be added without weakening the replay fence or teaching the companion to infer identity from pixels.

## Validate and install

```powershell
cargo run -p dcc-cua-cli -- profile validate examples/profiles/the-bazaar
cargo run -p dcc-cua-cli -- profile install examples/profiles/the-bazaar
```

Installation copies the declared package contents atomically to `~/.dcc-cua/profiles/the-bazaar`.

## Run the optional companion

Copy `companion/companion.example.json` to the user-owned configuration path
`~/.dcc-cua/config/profiles/the-bazaar/companion.json`, replace the three game
paths, and keep progress and storage observations local. Never place mutable configuration
inside the installed package: `profile install --replace` atomically replaces
that directory. Then:

```powershell
cargo run --manifest-path examples/profiles/the-bazaar/companion/Cargo.toml --release -- --config $env:USERPROFILE\.dcc-cua\config\profiles\the-bazaar\companion.json
```

Read state through the same CLI contract used by MCP or another agent platform:

```powershell
dcc-cua profile-state --profile-file ~/.dcc-cua/profiles/the-bazaar/profile.json
```

## Local card identity cache

The authoritative identity chain is `Player.log instance ID -> local GameData.db template UUID`. BazaarDB is an optional external reference on that exact local UUID, never the runtime authority for card text or patch behavior. Copy the reviewed seed to a user-owned cache and point `cardIdentityCachePath` at its absolute path:

```powershell
New-Item -ItemType Directory -Force $env:USERPROFILE\.dcc-cua\knowledge\the-bazaar
Copy-Item knowledge\card-identities.seed.json $env:USERPROFILE\.dcc-cua\knowledge\the-bazaar\card-identities.json
```

The companion loads and validates this file once, exposes cache hits as `externalReferences`, and never writes it. A record is accepted only when its local template exists, its canonical name exactly matches the current local database, its BazaarDB ID and URL are canonical, and it carries patch/date/match provenance. Add a mapping only after a current card is resolved locally and the external page is verified. Query the website only on a cache miss or when its recorded patch needs refreshing; do not make gameplay depend on an undocumented remote API. A stale external patch is useful reference metadata, not evidence that overrides local card data.

All input still goes through the DCC CUA Host with TaskGrant, fresh observation, PID, and HWND fences. The companion cannot click, unlock Windows, supply credentials, or bypass the visible control boundary.
