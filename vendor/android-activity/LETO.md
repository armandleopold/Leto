# android-activity, copied for Leto

android-activity 0.6.0 from crates.io, unchanged but for one function,
marked with a comment that starts with `Leto:`:

- `src/native_activity/input.rs`: `MotionEvent::summed_axis_value`. Android
  puts several reports of a mouse in one event, the last as the event's
  own values and the earlier ones as its "history", which this crate does
  not give. A mouse the window keeps for itself tells how far it moved in
  each report: read without the earlier ones, most of its way was lost and
  the PC's pointer crawled (seen on a Pixel 9 Pro, 2026-11-22).

Leto can go back to android-activity from crates.io once a release gives
an event's history.
