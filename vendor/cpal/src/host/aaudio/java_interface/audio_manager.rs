use super::AudioManager;

impl AudioManager {
    /// Get the AAudio mixer burst count from system property
    // Leto: read with the C library, not through Java's hidden
    // `SystemProperties`, whose call panicked in jni ("Expected an
    // exception after ExceptionCheck") on the thread a stream starts on.
    pub fn get_mixer_bursts() -> Result<i32, String> {
        let mut value = [0 as libc::c_char; 92]; // PROP_VALUE_MAX
        // SAFETY: the name ends with a nul byte, and the value is as large
        // as a property value can be.
        let length = unsafe {
            libc::__system_property_get(c"aaudio.mixer_bursts".as_ptr(), value.as_mut_ptr())
        };
        if length <= 0 {
            return Ok(2);
        }
        // SAFETY: the C library ends the value with a nul byte.
        let value = unsafe { std::ffi::CStr::from_ptr(value.as_ptr()) };
        value
            .to_string_lossy()
            .trim()
            .parse::<i32>()
            .map_err(|error| error.to_string())
    }
}
