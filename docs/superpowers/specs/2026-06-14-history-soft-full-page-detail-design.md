# History Soft Full Page Detail Design

## Context

The current history detail interaction feels disconnected when a user clicks `查看流程` from the history list. The detail screen replaces the list with a dark run-console experience, so the user loses visual continuity, list context, and a clear sense of where they are in the history workflow.

The selected direction is option C: a conventional full-page detail view, softened to match the existing light admin UI.

## Goals

- Keep the familiar `list -> detail -> back` pattern used by many admin products.
- Make the detail screen feel like a continuation of `歷史記錄`, not a separate tool.
- Preserve operational clarity: status, stage, duration, trigger, events, and errors remain easy to scan.
- Keep the existing frontend data flow and API calls.

## Scope

This change only affects the frontend history UI in `frontend/src/App.tsx` and `frontend/src/styles.css`.

In scope:

- Restyle `RunRoomDetail` into a light full-page history detail screen.
- Add breadcrumb-style context above the detail content.
- Make the back action more integrated with the history page.
- Keep the log area readable, with a restrained dark terminal block inside a light page.
- Improve responsive behavior for the detail header, summary facts, and log rows.

Out of scope:

- Backend API changes.
- Route or URL changes.
- New filters or search.
- Backup cancellation.
- Replacing the history table.

## Detail Layout

The selected history row opens a full-page detail view within the same main content area.

The page structure is:

- Breadcrumb row: `歷史記錄 / {configName} / {startedAt}`.
- Header card:
  - Left: `← 返回歷史記錄` button.
  - Main title: config name.
  - Supporting copy: running rows explain that events refresh every two seconds; completed rows explain that this is the execution record.
  - Right: status badge and optional `polling 2s` badge.
- Summary facts:
  - Status.
  - Current stage.
  - Elapsed time or duration.
  - Trigger source.
- Log card:
  - Light card shell titled `事件紀錄`.
  - Dark terminal-like inner area for event rows only.
  - Existing empty and error states remain visible inside this area.

This keeps the industry-standard full detail page while avoiding the current abrupt full-screen dark-mode transition.

## Interaction

- Clicking `查看流程` still calls the existing `inspect(row)` handler.
- `History` still renders `RunRoomDetail` when `selectedHistory` exists.
- Clicking `← 返回歷史記錄` still clears `selectedHistory` and returns to the current list state.
- Running backups still prefer `runningBackups` data and keep the existing two-second polling behavior.
- Downloads remain available from the history table only; no new download action is required in this redesign.

## Visual Direction

- Use the app's existing light moss/paper palette for the page shell.
- Keep rounded cards, soft borders, and subtle shadows consistent with `.panel` and `.metric`.
- Keep the log itself dark because event streams are easier to scan in a console block, but constrain darkness to the log body rather than the entire page.
- Use the existing `StatusBadge` style where practical so status vocabulary stays consistent across list and detail.
- Reduce uppercase English labels where they make the screen feel like a separate technical console; use Chinese labels for page-level UI and reserve mono styling for event rows.

## Responsive Behavior

- Desktop: breadcrumb, header card, four facts, and full-width log card stack vertically.
- Tablet: header content wraps; summary facts use two columns.
- Mobile: header actions stack; facts use one or two columns based on width; log rows stack into time, stage, and message lines.
- The detail page should scroll naturally with the app content instead of using a forced viewport-height console container.

## Error And Empty States

- If events are loading, the log heading shows `載入事件中`.
- If no events exist, the existing empty message remains.
- If `history.error_message` exists, it appears after the event rows in the log card.
- Global alert behavior for event loading failures remains unchanged.

## Accessibility

- The detail section keeps `aria-labelledby` pointing to the main heading.
- The back action remains a native button.
- The log retains `aria-live="polite"` so running events can be announced without being disruptive.
- Event rows keep readable text contrast in the dark log body.
- No icon-only controls are introduced.

## Verification

After implementation, verify:

- `npm run build` from `frontend`.
- Clicking `查看流程` opens a light, continuous detail page.
- Clicking `← 返回歷史記錄` returns to the same list state.
- Running backup events still refresh.
- Completed backup events and error messages remain readable.
- Desktop, tablet, and mobile layouts do not overflow.
