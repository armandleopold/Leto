# winit, copied for Leto

winit 0.30.13 from crates.io, unchanged but for an input hook on Android,
marked with comments that start with `Leto:`:

- `src/platform/android.rs`: `InputHook` and `set_input_hook`.
- `src/platform_impl/android/mod.rs`: `INPUT_HOOK`, asked first in
  `handle_input_event`; an event it takes goes no further.
- `src/platform_impl/android/mod.rs`: `is_mouse` and `mouse_events`. A
  mouse or a touchpad (a phone's desktop on a monitor has both) makes
  `CursorMoved`, `MouseInput` and `MouseWheel`, as on a desktop, rather
  than touches: winit made a finger of it, so the pointer only moved while
  a button was down, every button was the left one and the wheel did
  nothing.

winit makes touches of a gamepad's sticks (any motion event with a
pointer) and unknown keys of its buttons, and gives the app no way to see
them as a gamepad's. Leto's hook, `leto_system::gamepad::hear`, takes the
events whose source is a gamepad or a joystick.

Leto can go back to winit from crates.io once it tells of gamepads on
Android, or lets an app see the input events first, and tells of a mouse
as a mouse.
