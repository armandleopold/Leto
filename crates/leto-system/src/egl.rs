//! A picture of the graphics card's, from a decoder's [`Reader`], as the
//! texture of the window's OpenGL ES: through EGL, which draws the window.
//!
//! [`Reader`]: crate::decoder::Reader

use std::{
    ffi::{CStr, c_void},
    sync::OnceLock,
};

use crate::decoder::Image;

type Display = *mut c_void;
type EglImage = *mut c_void;

/// `GL_TEXTURE_EXTERNAL_OES`: a texture the card reads as the buffer says,
/// turning a picture's brightness and colours into pixels as it does.
pub const TEXTURE_EXTERNAL: u32 = 0x8D65;

const NATIVE_BUFFER_ANDROID: u32 = 0x3140;
const IMAGE_PRESERVED: i32 = 0x30D2;
const TRUE: i32 = 1;
const NONE: i32 = 0x3038;

type CurrentDisplay = unsafe extern "C" fn() -> Display;
type ClientBuffer = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type CreateImage =
    unsafe extern "C" fn(Display, *mut c_void, u32, *mut c_void, *const i32) -> EglImage;
type DestroyImage = unsafe extern "C" fn(Display, EglImage) -> u32;
type TargetTexture = unsafe extern "C" fn(u32, EglImage);

/// What EGL is asked, found once.
struct Calls {
    current_display: CurrentDisplay,
    client_buffer: ClientBuffer,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    target_texture: TargetTexture,
}

fn calls() -> Option<&'static Calls> {
    static CALLS: OnceLock<Option<Calls>> = OnceLock::new();
    CALLS
        .get_or_init(|| {
            // SAFETY: the system's EGL, which the window already loaded;
            // each symbol is cast to the type EGL's headers give it.
            unsafe {
                let egl = libc::dlopen(c"libEGL.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD);
                let egl = if egl.is_null() {
                    libc::dlopen(c"libEGL.so".as_ptr(), libc::RTLD_NOW)
                } else {
                    egl
                };
                if egl.is_null() {
                    return None;
                }
                let symbol = |name: &CStr| {
                    let found = libc::dlsym(egl, name.as_ptr());
                    (!found.is_null()).then_some(found)
                };
                let proc_address: unsafe extern "C" fn(*const libc::c_char) -> *mut c_void =
                    std::mem::transmute(symbol(c"eglGetProcAddress")?);
                let extension = |name: &CStr| {
                    let found = proc_address(name.as_ptr());
                    (!found.is_null()).then_some(found)
                };
                Some(Calls {
                    current_display: std::mem::transmute::<*mut c_void, CurrentDisplay>(symbol(
                        c"eglGetCurrentDisplay",
                    )?),
                    client_buffer: std::mem::transmute::<*mut c_void, ClientBuffer>(extension(
                        c"eglGetNativeClientBufferANDROID",
                    )?),
                    create_image: std::mem::transmute::<*mut c_void, CreateImage>(extension(
                        c"eglCreateImageKHR",
                    )?),
                    destroy_image: std::mem::transmute::<*mut c_void, DestroyImage>(extension(
                        c"eglDestroyImageKHR",
                    )?),
                    target_texture: std::mem::transmute::<*mut c_void, TargetTexture>(extension(
                        c"glEGLImageTargetTexture2DOES",
                    )?),
                })
            }
        })
        .as_ref()
}

/// Whether the window's EGL takes the decoder's pictures as textures.
pub fn takes_pictures() -> bool {
    calls().is_some()
}

/// A picture of the card's, made the texture bound to [`TEXTURE_EXTERNAL`]
/// when it was made. It stops being one when dropped, which must happen
/// with the context it was made with current, before the picture goes.
pub struct Texture {
    display: Display,
    image: EglImage,
}

impl Texture {
    /// Makes `picture` the texture now bound to [`TEXTURE_EXTERNAL`].
    ///
    /// # Safety
    ///
    /// The window's context is current on this thread, a texture is bound
    /// to [`TEXTURE_EXTERNAL`], and `picture` outlives what is returned.
    pub unsafe fn bind(picture: &Image) -> Result<Self, String> {
        let calls = calls().ok_or("EGL does not take the decoder's pictures")?;
        // SAFETY: as the caller promises; the buffer is the picture's.
        unsafe {
            let display = (calls.current_display)();
            let buffer = (calls.client_buffer)(picture.buffer().cast_const());
            if buffer.is_null() {
                return Err("EGL did not take the picture's buffer".into());
            }
            let attributes = [IMAGE_PRESERVED, TRUE, NONE];
            let image = (calls.create_image)(
                display,
                std::ptr::null_mut(),
                NATIVE_BUFFER_ANDROID,
                buffer,
                attributes.as_ptr(),
            );
            if image.is_null() {
                return Err("EGL made no image of the picture".into());
            }
            (calls.target_texture)(TEXTURE_EXTERNAL, image);
            Ok(Self { display, image })
        }
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if let Some(calls) = calls() {
            // SAFETY: made in `bind`, destroyed once, as its maker promised.
            unsafe { (calls.destroy_image)(self.display, self.image) };
        }
    }
}
