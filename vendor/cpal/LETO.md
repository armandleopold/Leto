# cpal, copied for Leto

cpal 0.18.2 from crates.io, unchanged but for two functions, each marked with
a comment that starts with `Leto:`:

- `src/host/aaudio/java_interface/audio_manager.rs` (`get_mixer_bursts`):
  the AAudio mixer burst count is read with the C library's
  `__system_property_get`, not through JNI and Java's hidden
  `android.os.SystemProperties`. Through JNI it panicked in jni 0.22
  ("Expected an exception after ExceptionCheck") right after the sound
  output opened, on the thread a stream starts on: no stream ever started
  on Android (seen on the emulator, 2026-11-21).

- `src/host/aaudio/mod.rs` (`configure_for_device`): every stream asks
  for `AudioPerformanceMode::LowLatency`, which cpal only asks for with its
  `realtime` feature (that also pulls ALSA and thread-priority crates). On
  a Fire TV the default path played the PC's sound far behind the picture
  (Armand, 2026-11-22).

Leto can go back to cpal from crates.io once a release reads the property
without Java, or the panic is understood and fixed upstream.
