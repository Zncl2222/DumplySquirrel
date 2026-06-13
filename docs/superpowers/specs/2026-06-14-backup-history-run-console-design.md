# Backup Settings Scroll And History Run Console Design

## Context

The current UI has two issues:

- The backup settings slide-in panel can become taller than a small browser viewport, but the user cannot clearly scroll the side panel content.
- The history detail view spends too much space on the left-side visual process area. The user wants to see `LIVE LOG` and backup status directly, with a clear way to return to the history list.

The selected direction is option A: a compact run console.

## Scope

This change only affects the frontend UI in `frontend/src/App.tsx` and `frontend/src/styles.css`.

In scope:

- Make the backup settings panel usable in short and narrow viewports.
- Redesign the history detail screen into a compact run console.
- Keep existing polling, event loading, download, pagination, and backup trigger behavior.

Out of scope:

- Backend API changes.
- New history filters or search.
- Backup cancellation.
- Replacing the app-wide navigation or table structure.

## Backup Settings Panel

The slide-in panel keeps the existing header, form body, and footer structure.

The panel should behave as a viewport-bound layout:

- The panel height is constrained to the dynamic viewport.
- The header stays at the top.
- The form body owns vertical scrolling.
- The footer stays visible at the bottom so `取消`, `建立設定`, and `更新設定` remain reachable.
- On mobile widths, the panel uses the full viewport width and still scrolls internally.

This fixes the small-window scrollbar issue without changing form fields or save behavior.

## History Detail Layout

The selected history row opens a single-page run console:

- Top bar:
  - A clear `返回歷史列表` button on the left.
  - The backup config name and short status text next to it.
  - A `polling 2s` badge when the backup is running.
- Status summary:
  - Four compact facts: status, current stage, elapsed/duration, trigger.
  - The facts appear above the log so the user sees state before reading events.
- Main content:
  - `LIVE LOG` is the largest visual area.
  - Log rows keep time, stage, and message columns on desktop.
  - Log rows stack on small screens to prevent text overflow.
  - Error message remains visible after the event list.

The large canopy/path visualization is removed from the detail screen because it competes with the operational information the user needs most.

## Data Flow

No API contract changes are needed.

- `History` continues to select a row with `inspect(row)`.
- Running rows still prefer `runningBackups` data when available.
- `RunRoomDetail` receives the current history row, config name, events, loading state, and `onBack`.
- `stageFromEvents`, `stageLabel`, and `runDuration` continue to derive display values from the existing history and event data.
- Polling stays at the existing two-second interval for running backups.

## Error And Empty States

- If events are still loading, the log header shows the loading label.
- If no events exist yet, the log area shows the existing empty message.
- If `history.error_message` exists, it appears in the log area as an error block.
- The global alert behavior remains unchanged for event loading failures.

## Accessibility

- The return action remains a native button.
- The detail section keeps a stable heading through `aria-labelledby`.
- The log keeps `aria-live="polite"` so new running events can be announced without being disruptive.
- Icon-only controls are not introduced.
- Keyboard focus behavior remains native for the detail view.

## Responsive Behavior

- Desktop: status summary appears above a full-width log panel.
- Tablet and mobile: the heading wraps vertically as needed, status facts use fewer columns, and log rows stack.
- The page uses dynamic viewport sizing where relevant to avoid mobile browser chrome clipping.
- Text in buttons, facts, and log rows should wrap or truncate cleanly without overlapping.

## Verification

Run frontend verification after implementation:

- `npm run build` from `frontend`.
- Manual viewport checks for:
  - Backup settings panel at short viewport height.
  - Backup settings panel at mobile width.
  - History detail return button visibility.
  - History detail status summary and log readability on desktop and mobile.
