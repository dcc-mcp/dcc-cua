---
name: the-bazaar-cua
description: Operate The Bazaar through an exact-window dcc-cua Host session using verified inventory, shop, reward, and encounter interactions.
---

# The Bazaar via dcc-cua

## Mandatory startup context

Before opening a Host session, match the exact application and load the
profile-owned startup documents fenced to the installed data snapshot:

```powershell
dcc-cua profile match --app TheBazaar.exe --title "The Bazaar"
dcc-cua profile context --id the-bazaar --identity "game-data=sha256:<GameData.db SHA-256>" --selector "character=Pygmalien"
```

Read every returned document before observing the game. When
`requiresRefresh` is true, do not apply stale build, shop, or encounter advice.
A maintenance workflow may generate documents under
`~/.dcc-cua/knowledge/the-bazaar/`; gameplay never scrapes the web or rewrites
knowledge on its hot path.

Use `profile.json` to match `TheBazaar.exe`. Keep one exact PID/HWND Host session, live observation, shared-memory frames, and the custom CUA theme active. Never reuse coordinates after the window bounds or observation ID changes.

## Semantic state first

Before reasoning from pixels, call `dcc-cua profile-state --profile-file ~/.dcc-cua/profiles/the-bazaar/profile.json`. The optional `bazaar-agent` source is loopback-only, read-only, schema-pinned to `2.5.0`, ETag-aware, and bounded to a one-second/one-MiB read. When it is unavailable, the command degrades to `visual_cua`; do not install or restart a companion in the middle of an active run.

Treat `tickId` as the semantic observation fence. Recompute only when the tick changes. Build one card-instance graph from `boardItems`, `chestItems`, `playerSkills`, `selectionOptions`, and `availableActions`; never merge cards by display name. Each node must retain instance ID, template ID, tier, enchantment, attributes, active abilities, size, socket span, sell value, and tags.

Read `stateCompleteness` before using any field. `verified` data may drive a transaction; `partial` data may constrain a decision but cannot prove absence; `unknown` data must be inspected. `run.progress` is an explicitly sourced volatile observation because this Player log does not expose reliable economy fields. Refresh that snapshot after gold, health, level, day, hour, wins, or losses change; never treat an old observation ID as live state.

Read `run.choiceKind` before interpreting any selectable art. `encounter` means the top-level hourly adventure layer: every visible card is a selectable encounter entry, such as Nautica, Street Celebration, or Rest. It is not an item or loot offer. Only after selecting the entry can `event_option`, `loot_reward`, `level_up_reward`, or `item_operation` appear. `level_up_reward` is only the parent layer: its options may be an item, loot, skill trainer, item upgrade, enchantment, or another event, and a trainer can then deal concrete skill cards. Read each option's `selectionCategory` and preserve the full parent/child receipt; shared card-shaped presentation does not imply shared semantics.

For a resolved local template, reuse `externalReferences` instead of reading the same description again. The external cache is an optional BazaarDB alias keyed by the local GameData template UUID. Local GameData and Player.log remain authoritative; a cache hit saves identification work, while a cache miss or patch mismatch may trigger one reviewed lookup. Never merge by artwork, translated title, or external ID alone, and never let an older BazaarDB patch override current local attributes.

Read `decisionSupport` as a deterministic guard, not an autopilot score. Its priority is current verified run state, active relationship graph, recent combat evidence, and only then the ten-win corpus. Ten-win rows are population priors, never route authority. A named archetype may suggest candidates, but it cannot justify breaking an active edge, exceeding the spend budget, taking an unsafe level gap, or selecting an unresolved card.

For every board mutation, derive edges for `left`, `right`, `adjacent`, `leftmost`, `rightmost`, tag-trigger subjects, charge/haste targets, and random targets. Recompute those edges after every move because positional text is an executable rule, not presentation metadata. Score a shop/reward/skill candidate by the delta against the current graph: damage/heal/shield over time, first-activation timing, charge/cooldown effects, Crit concentration, control, survivability, socket opportunity cost, displaced triggers, and gold. Reject a candidate whose nominal rarity rises while the measured graph value falls.

Classify every owned card into one or more explicit activation scopes: `board` combat-active, `stash` passive, `owned` global/event-triggered, or inert inventory. Preserve explicit triggers such as `start_of_day`, `continuous_in_stash`, and `on_sell`. A stash card is not automatically inert and a generic board aura is not automatically active from the stash; only current-build text or an authoritative observed trigger can widen its scope. Block selling an active stash/owned edge until its value has been realized or a strictly better verified gain exceeds it.

## Exact identity and relationship gate

Never transact from artwork, color, screen position, or a remembered offer. Before selecting a shop item, reward, skill, or encounter, resolve every candidate to an exact current-build identity and effect from at least one authoritative source: typed profile state, exact instance/template ID in `Player.log`, or a fully read tooltip joined to the read-only `GameData.db`. A partial tooltip or visual resemblance is insufficient. If any candidate remains unresolved, inspect again or leave the choice unresolved; do not click by elimination or positional guess.

Represent the build as an executable relationship graph rather than a flat tier list. Every effect edge has `source`, `trigger`, `target selector`, `resolved targets`, `value`, and `state` (`active`, `dormant`, or `broken`). A skill with no trigger source has zero current-board value even if it is generically strong. For example, `Quick Freeze` is dormant while the board has no Haste source. An exactly-one-Weapon condition becomes broken as soon as a second Weapon is bought or placed. An item that is both the leftmost and rightmost Weapon receives both independent Honing Steel effects.

Before any irreversible choice, enumerate all exact candidates, recompute the graph for each hypothetical result, and persist the winning explanation as: retained active edges, added active edges, lost/broken edges, dormant future edges, capacity/gold delta, and expected combat delta. Selection is blocked until this comparison exists. After selection, verify the exact authoritative command and recompute from the resulting board; input delivery alone is not success.

Resolve retained skills from `playerSkillInstances` and exact receipts. Treat configured names as a partial bootstrap only; a skill whose instance or trigger source is unresolved cannot justify a transaction.

## Verified interactions

Coordinate clicks already move the system pointer. Read `rightClickBehavior` before using right-click: `preview` is inspectable, `selects_candidate` is irreversible, and unknown means do not use it for inspection. For a preview-safe card, use one `right-click` with `capture_after`, then use that returned post-snapshot as the next observation fence. Issue a separate `move` only when the game must establish hover state before the preview or when information exists exclusively in hover state. Close a preview with one right-click on a verified blank region and verify absence from its post-snapshot. Do not press Escape while the pointer still hovers the card because the application can immediately reopen or cycle its preview. Choice cards use one click from the latest resolved frame, followed by server-authoritative verification; do not pre-focus, pre-move, or double-click by default.

1. Backpack: execute target `inventory/backpack` action `toggle`; `profile.json` resolves it to `Space`. Verify the storage row appears or disappears.
2. Sell: open the backpack, move the intended item to the packed right edge, then read the fresh frame because the board repacks by item size. Re-drag the same item in place at its new visible center and immediately press the configured `Z` sell binding. Verify an exact-instance `SellCardCommand`, item removal, and gold increase. Hover alone and dragging toward a merchant header are not verified sale mechanisms.
3. Buy: for a new item, drag the shop card into an explicit size-aware board or stash target and verify gold changed plus `SelectItemCommand`. For a verified duplicate that upgrades an owned instance, click the duplicate and verify the same owned instance is purchased again, the offered duplicate is disposed, and `tierProvenance=player_log_repeat_purchase_upgrade`; do not drag a duplicate as though it were a new card.
4. Claim a free reward: first count size-aware free sockets across the board and storage, then identify the actual generated reward card in the state-specific play region and drag it toward the intended free region. A large decorative header or chest illustration at the top of `LootState` is not the reward. Verify the exact instance moved to a `PlayerSocket` or `PlayerStorageSocket`. When both surfaces are full, sell or move an owned item before retrying.
5. Select an encounter: in `choiceKind=encounter`, treat all visible cards as selectable adventure entries. Click the chosen encounter card once and wait for the server-authoritative state transition before interpreting any nested offer or sending another input.
6. Reroll: inspect the top reroll control, record its cost, then click only while a current-graph need is unmet, `decisionSupport.spendBudget` covers the cost, and the per-shop `shopRerollCap` is not exhausted. Verify `RerollCommand`, changed offers, and changed gold. If any postcondition is unchanged, count no success but stop retrying until the control state is re-resolved.
7. Day/hour: read the center number of the left compass as Day and its lit outer progression marks as Hour. Cross-check it with the next `ChoiceState` or authoritative log transition; never infer either value from elapsed wall time.
8. Event candidate: selecting an encounter can deal a second-stage item or skill candidate. Inspect that candidate and explicitly claim it when wanted. The top-right arrow exits the event and disposes an unclaimed candidate; an "upgrade an item" description does not prove that the target is selectable.
9. Rune or skill reward: a dealt rune/skill choice is pending until one exact candidate is selected and `SelectSkillCommand` succeeds. A `skl_` candidate has `selectionCategory=skill` and `rightClickBehavior=selects_candidate`; right-click commits the choice rather than opening a harmless preview. Inspect through typed/local identity or a non-committing hover path only. Verify the exact selected instance, new skill icon, and authoritative state transition before proceeding.
10. Level-up reward chain: selecting a level-up chest/card is only the first step. Follow every nested state (`EncounterState`, `PedestalState`, or equivalent). If it deals an item, skill, rune, or loot card, explicitly claim that generated reward and verify `SelectItemCommand` or `SelectSkillCommand` plus placement. In the observed `PedestalState`, drag the intended owned item from its current board/stash position into the center pedestal; do not click the item and do not drag the catalyst art onto the item. Verify the authoritative commit plus changed tier/enchantment before continuing. The chain is complete only after all generated outcomes are verified and the state returns to `ChoiceState`. Never choose the next hourly encounter while a generated reward or unapplied pedestal is still visible.

## Mandatory pending-reward gate

Before every hourly choice and after every encounter, combat reward, event, or level-up transition, check for pending rune/skill icons, generated item/loot cards, and unapplied item operations. Clear all wanted pending rewards and verify their exact-instance placement or changed stats before advancing. Treat `LevelUpState`, a nested reward `EncounterState`, `PedestalState`, a visible generated card, or an unresolved upgrade preview as `pending_reward=true`; this is a hard gate equal to the post-combat record gate. Persist one receipt to `~/.dcc-cua/knowledge/the-bazaar/rewards.jsonl` containing the selected option, every generated/applied outcome, exact instances, authoritative commands, completion transition, and evidence frame.

Also inspect authoritative `Cards Spawned` transitions at the start of each day and after combat. Passive producers such as Fishing Net can place generated Aquatic/Loot items directly on the board without a claim screen. Identify the exact spawned instance, inspect its current-build text, persist it as a reward receipt with its producer and placement, then recompute the board rather than treating it as an unexplained owned item.

## Required combat review

After every combat, stop on the replay screen, open the magnifier, and save the record frame before pressing Continue. For PvP, result is required: record the compass day/hour, opponent, wins and prestige before/after, both health totals, each item's trigger count, direct damage, burn/heal/shield, and control totals. Resolve the result only from an authoritative winner/loser field, wins/prestige delta, explicit verdict, or direct user confirmation. For PvE, the result itself is optional; record only measured output deficits, loot identity, and reward-chain completion. A replay health bar shows the combatant's maximum/statistical presentation, and day income or PvE reward gold is not a winner signal. Append one verified line to `~/.dcc-cua/knowledge/the-bazaar/battles.jsonl`, then state the largest measured deficit before choosing the next encounter or purchase. A later run-summary screen cannot reconstruct per-item combat totals, so Continue is a hard gate until the required record artifact exists.

For the current build, the record screen is the authoritative per-fight learning source. `Player.log` only proves state transitions and commands; it does not prove who won. If BazaarPlusPlus is installed, prefer its loopback-only schema `2.2.0` context and local battle SQLite for typed state/history, while retaining dcc-cua for exact-window capture, visible control, recording, and any action the companion does not expose. Never install or enable a mod during an active run without an explicit restart decision.

## Decision loop

- Use the latest frame before every action and only its observation ID. Prefer the fresh `post_snapshot` returned by `capture_after`; request a standalone snapshot only when no action result supplied one, the frame is stale, or out-of-band game state changed.
- Do not issue a standalone `move` before a coordinate click/right-click/drag unless an unresolved hover-only state is required. The default interaction budget for a resolved choice is one action plus its post-snapshot.
- Read the local `GameData.db` in read-only mode when exact current-build item text is needed.
- Use current build references for composition priorities, but prefer the strongest synergy already present over forcing a named build. Never chase a ten-win route after its enabling pieces, economy window, or board capacity have passed.
- Adapt by measured deltas. Preserve healthy active edges while testing direct upgrades; repair broken edges first; pivot only when the immediate verified graph improves or repeated PvP records identify a deficit the current core cannot fix. Derive run phase from verified day, wins, and prestige. Loss streak is supporting evidence only and cannot override the compass: at critical prestige, guaranteed next-PvP survival outranks speculative scaling.
- Treat `decisionSupport.maxSafeOpponentLevel` as the default encounter ceiling. Exceed it only when a recent combat record demonstrates sufficient margin against the relevant mechanic; rarity or reward value alone is not evidence.
- On encounter choice, resolve each card once, then apply `decisionSupport.encounterPolicy`: discard candidates above the safe ceiling, rank the rest by immediate active-relationship gain, measured combat-deficit repair, survival margin, and only then reward value. If none is eligible, take the lowest measured risk. Do not re-inspect an unchanged card or keep pursuing a preselected position/build.
- After every mutation, verify in the next frame or `Player.log`. A Host input result proves delivery, not game state.
- Before every reward, purchase, or board move, compute size-aware capacity for both 10-socket surfaces. A rejected claim is a capacity failure until the frame or log proves otherwise.
- Preserve proven category synergies over nominal rarity. Query a dealt event item in the read-only `GameData.db`, calculate the board-size and category opportunity cost, and use the event exit when the offered item lowers the measured build.
- Score adjacency from confirmed card text: enchantments and left/right effects are part of the board state, not cosmetic metadata. Recheck multicast or stat labels after every move.
- Treat a skill or item whose required trigger is absent as dormant, and rank it below an immediately active edge unless a verified same-chain choice supplies that trigger. Never value a future combo as though it is already active.
- Before choosing a skill, item, or encounter, write the exact identity/effect table for every candidate that remains decision-relevant. A guaranteed option may be selected without opening lower-ranked unknowns when verified progress policy already proves it dominates them; persist that short-circuit reason to avoid redundant inspection.
- If a postcondition fails, refresh the observation and reassess capacity, hover state, or server delay. Do not blindly retry.
- Treat every action result as a possible new fence even when `capture_after=false`. If the Host reports `observation_required` or an animation can advance state, consume or request a fresh observation before the next action; never assume a no-capture action preserves the prior observation ID.

## Current interaction boundary

The Profile owns application vocabulary and verified sequences. The Host owns authorization, PID/HWND fences, capture, input delivery, interruption, banner, cursor, and recording. Secure-desktop unlock and credentials remain outside this Profile.
