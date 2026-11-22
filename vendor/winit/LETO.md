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
- `src/platform_impl/android/mod.rs`: `key_at`. A key's `physical_key`
  comes from its scan code (the Linux input code of where it sits) for the
  keys whose letter changes with the layout, rather than from Android's key
  code, which is the letter of the layout Android was told the keyboard
  has. A PC typed Q for an AZERTY keyboard's A once Android knew it was
  French.

winit makes touches of a gamepad's sticks (any motion event with a
pointer) and unknown keys of its buttons, and gives the app no way to see
them as a gamepad's. Leto's hook, `leto_system::gamepad::hear`, takes the
events whose source is a gamepad or a joystick.

Leto can go back to winit from crates.io once it tells of gamepads on
Android, or lets an app see the input events first, and tells of a mouse
as a mouse.
