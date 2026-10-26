# HOT Apply UI source-only handoff

The IPC result retains `ok`, `status`, and optional nullable `applyId`; adds optional
`transactionId`, `runtimeGeneration`, `selectionIntentGeneration`, `restartConfirmation`, `reasons: {code,path}[]`, `pendingApply`, and `restartRequired`.
Statuses are `applied`, `deferred`, `skipped`, `hotApplied`,
`hotDurabilityPending`, `restartRequired`, `conflict`, `rejected`, and `unconfirmed`.
`api.proxy.applyPendingChanges(restartConfirmation?: string)` omits the argument bag
on the original call and sends `{restartConfirmation}` only on explicit confirmation.

Both desktop and mobile use the same localized Modal. The first restart-required
response opens the prompt; cancel/close does not call Apply. Confirmation forwards
only the returned token through the store and never saves newer drafts. A stale token
conflict remains visibly failed/pending; it does not auto-confirm a newer candidate.
No usable token means unconfirmed failure, never restart permission.

HOT committed receipts settle without a process lifecycle event. `applied` and
`deferred` retain existing apply-ID attribution, early-event buffering, explicit
stop cancellation, and timeout behavior. Durability-pending has a persistent truthful
runtime-applied/persistence-pending bar without a replay button. The transaction-bound `api.proxy.runtimeApplyStatus(transactionId)` command now
reconciles that state automatically through saved original transaction reads. Only
hotDurabilityPending/unconfirmed receipts with an explicit transaction ID start
polling. Reads are serialized at 2-second intervals, never replay Apply, reject
mismatched transaction IDs, preserve failed/ambiguous observations, and stop on
unmount, stop, newer attempt, changed generation/intent or terminal authoritative
receipt. Only matching hotApplied confirms durability and clears the pending state.
The backend owns exact kernel transaction/incarnation/control binding. Token-bearing
receipts echo authoritative runtimeGeneration and selectionIntentGeneration; UI
polling requires safe integer owner values and never infers the owner from pre-save
state. Every accepted status response must match the original transaction and both
echoed owner values. Frontend request identity, running state, observed generation
and available current selection-intent guards also reject stale responses. Missing
token or authoritative owner evidence means no polling, not invented identity.

Only trusted reason codes `structuralChange` and `unsupportedCore` are translated.
Structural path mappings cover TUN, listener ports, network namespace, services and
cache path; unknown paths get generic structural wording. No raw reason, arbitrary
kernel message, or raw path is rendered. All five shipped main locales have the same
new keys. `classifyStaged.pendingApply` now drives saved-but-pending labels; absent
classification evidence stays unknown rather than falling back to restartRequired.

## Frozen UI paths

- src/contracts/types.ts
- src/contracts/staged-classification.test.ts
- src/ipc/api/proxy.ts
- src/domain/ipc-channels.ts
- src/store/staged-config-store.ts
- src/store/staged-config-store.test.ts
- src/components/layout/pending-bar-logic.ts
- src/components/layout/pending-bar-logic.test.ts
- src/components/layout/runtime-apply-polling.ts
- src/components/layout/runtime-apply-polling.test.ts
- src/components/layout/use-runtime-apply-reconciliation.ts
- src/components/layout/PendingChangesBar.tsx
- src/mobile/MobilePendingBar.tsx
- src/components/dialogs/RestartApplyDialog.tsx
- src/components/dialogs/restart-apply-reasons.ts
- src/components/dialogs/restart-apply-reasons.test.ts
- src/components/dialogs/app-transactions.browser.test.ts
- src/i18n/locales/en-US.json
- src/i18n/locales/zh-CN.json
- src/i18n/locales/zh-TW.json
- src/i18n/locales/ru.json
- src/i18n/locales/fa.json
- HOT_APPLY_SOURCE_HANDOFF.md

Authored coverage: receipt semantics, unconfirmed/non-success outcomes, durability
no-replay state, token preservation without save, stale-token preservation of drafts,
shared desktop/mobile confirmation and decline, safe reason mapping and locale parity.
Reconciliation fixtures cover exact transaction reads, transport ambiguity, mismatched
receipt IDs, one in-flight read, cancellation and stale late completion, plus mounted
desktop/mobile durability completion and stop-before-readback.
No tests, generators, builds, native execution, browser execution, CI or pushes ran.
The source edits require authorized TypeScript/browser/store regression execution;
this handoff is not a test-pass or end-to-end HOT claim.

Final source alignment additions (unexecuted): RuntimeApplyResult exposes optional persistenceMode; exact OWNER_STATE_SUPERSEDED/desired conflicts use the localized latest-settings restart-source message on initial result and status polling. Matching-transaction terminal conflict/rejected/skipped stops polling without requiring success owner fields; success and pending publication still require authoritative generation/intent. Mesh runtimeCommitted evidence is accepted as loaded while retaining scope/staleness/version gates; it is not persistence proof. Focused report/badge, five-locale, conflict-reason and polling source tests were authored. No generators, builds, tests or CI ran.
