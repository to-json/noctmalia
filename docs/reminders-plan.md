# noctmalia — reminders

Date: 2026-09-14
Depends on: None

---

## Context

The calendar Experiment already fires `calendar.items.onAlarm` (`tbd/bridge/background.js:356`,
schema at `tbd/bridge/experiments/calendar/schema/calendar-items.json:209`) whenever Thunderbird's
own `calIAlarmService` decides an event's reminder is due, with the item and alarm payload already
converted (`convertAlarm`, `ext-calendar-utils.sys.mjs:226`). `src/ical.rs` already round-trips
`VALARM` as `alarms: Vec<Duration>`, and the event editor (`src/surfaces/calendar.rs`) already lets
you toggle "remind me N minutes before." What's missing is entirely on our side: nothing subscribes
to the event, so a reminder firing produces no visible effect today.

This is the smallest plan in the sequence and ships first for that reason — it's a straight wire-up
of an existing bridge event to a new small notification, with no architecture to invent.

Two things are settled as fact rather than open risks, checked against the Experiment source before
writing this plan:
- **No snooze call exists.** `ext-calendar-items.js` only exposes `onAlarm`/`onAlarmsLoaded`/
  `onRemoveAlarmsByItem`/`onRemoveAlarmsByCalendar` — nothing to re-fire an alarm later. The toast
  offers dismiss only; a "snooze" button is cut, not deferred.
- **`tools/fake-bridge.py` has no calendar-alarm synthesis today** (only `onCreated`/`onUpdated`/
  `onRemoved` for calendar items). Adding it is Stream 1 work, not a risk to discover later.

## Stream 1: Subscribe and surface

**Problem:** `calendar.items.onAlarm` events arrive over the bridge and are dropped.

**Files:** `crates/noctmalia/src/calendar.rs`, `crates/noctmalia/src/app.rs`, `tools/fake-bridge.py`

### 1.1 Route the event
Add the alarm event to the bridge event enum/dispatch (wherever `onCreated`/`onUpdated` land today)
and turn it into an app `Message`.

### 1.2 Show it
A toast/banner consistent with the existing notice-bar pattern (README: "the notice banner slides
open"), naming the event, with a dismiss action. No snooze — see above.

### 1.3 Fake-bridge support
Add `onAlarm` synthesis to `tools/fake-bridge.py`'s calendar handling so a test (and `just fake`)
can trigger one without a real Thunderbird.

### 1.4 Test
Extend `tests/calendar.rs` to fire a synthetic `onAlarm` through the fake and assert the app
produces the toast state.

## Sequence integration

No dependency in or out. Ships first.

## Risks

- None beyond the ones already resolved above by checking the Experiment source ahead of writing
  this plan.
