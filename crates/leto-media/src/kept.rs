//! Pictures that stay on the graphics card, from the decoder to the screen.
//!
//! A picture decoded on the graphics card used to come back to memory, as
//! planes, and go to the card again when the window drew it: three copies
//! of six megabytes for a picture of 3840 × 1080, and a decoder that waits
//! each time until the card has finished the picture, which is where most
//! of the time of decoding went. Moonlight never takes a picture off the
//! card, and neither does this.
//!
//! The decoder's pictures are Direct3D's and the window draws with OpenGL.
//! The decoder has the card copy each picture into a texture of Leto's and
//! goes on to the next: it waits for nothing. When the window is about to
//! draw, the card's own video processor turns the latest picture into
//! pixels in another texture of Direct3D's, and the window's OpenGL is
//! given that texture to draw from (`WGL_NV_DX_interop2`, which the drivers
//! of AMD, Intel and Nvidia all have). Nothing of the picture is in memory.
//!
//! On Android, the decoder gives its pictures onto an image reader's, on the
//! card, and the window's OpenGL ES draws each straight from there, as a
//! texture the card itself turns into pixels (`samplerExternalOES`).
//!
//! Elsewhere a [`Kept`] cannot be made, and pictures are planes as before.

/// A picture that stayed on the graphics card. It stands for the latest one
/// the decoder left there: showing it shows that one.
#[derive(Clone)]
pub struct Kept {
    #[cfg(windows)]
    pub(crate) shared: std::sync::Arc<windows::Shared>,
    #[cfg(target_os = "android")]
    pub(crate) shared: std::sync::Arc<android::Shared>,
    /// Nothing makes one where pictures cannot stay.
    #[cfg(not(any(windows, target_os = "android")))]
    never: std::convert::Infallible,
}

impl std::fmt::Debug for Kept {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a picture on the graphics card")
    }
}

impl PartialEq for Kept {
    /// Whether both are on the same texture of the card.
    fn eq(&self, other: &Self) -> bool {
        #[cfg(any(windows, target_os = "android"))]
        return std::sync::Arc::ptr_eq(&self.shared, &other.shared);
        #[cfg(not(any(windows, target_os = "android")))]
        {
            let _ = other;
            self.never()
        }
    }
}

#[cfg(not(any(windows, target_os = "android")))]
impl Kept {
    /// There is none, so this is never reached.
    pub(crate) const fn never(&self) -> ! {
        match self.never {}
    }
}

impl Eq for Kept {}

#[cfg(windows)]
pub(crate) mod windows {
    use std::{
        ffi::{CStr, c_char, c_void},
        mem::ManuallyDrop,
        sync::{
            Arc, Mutex, MutexGuard, PoisonError,
            atomic::{AtomicU64, Ordering},
        },
    };

    use glow::HasContext as _;
    use windows::{
        Win32::{
            Foundation::{GetLastError, HANDLE, RECT},
            Graphics::{
                Direct3D11::{
                    D3D11_BIND_DECODER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
                    D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
                    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
                    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
                    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
                    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
                    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
                    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext,
                    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoContext1, ID3D11VideoDevice,
                    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
                    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
                },
                Dxgi::Common::{
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601, DXGI_FORMAT,
                    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_RATIONAL,
                    DXGI_SAMPLE_DESC,
                },
                Gdi::HDC,
                OpenGL::{wglGetCurrentDC, wglGetProcAddress},
            },
            Media::MediaFoundation::{IMFDXGIBuffer, IMFSample},
        },
        core::{BOOL, Interface as _, PCSTR, s},
    };

    use super::Kept;

    type Gl = glow::Context;
    type Texture = <Gl as glow::HasContext>::Texture;

    /// What both sides use, made once for pictures of one size.
    ///
    /// The decoder copies each picture into `picture` as it decoded it: a
    /// copy on the card, asked for and not waited for. The window has the
    /// card's video processor turn the latest one into `pixels` when it is
    /// about to draw, and draws from those. So the decoder never waits for
    /// the window, and a picture the window never draws is never turned
    /// into pixels.
    ///
    /// Moonlight copies the same way where it does not draw from the
    /// decoder's own textures (`renderVideo` in moonlight-qt's
    /// `d3d11va.cpp`): the decoder then has its texture back at once.
    pub struct Shared {
        /// The card as Direct3D knows it. Kept for as long as the textures
        /// are, which can be longer than the decoder lives.
        device: ID3D11Device,
        /// What the card is asked through, and the same for video.
        commands: ID3D11DeviceContext,
        video: ID3D11VideoContext,
        /// The latest picture, in the planes the decoder made.
        picture: ID3D11Texture2D,
        /// How many pictures were copied into `picture`.
        copied: AtomicU64,
        processor: ID3D11VideoProcessor,
        /// What the processor was made from, kept for as long as it is.
        _enumerator: ID3D11VideoProcessorEnumerator,
        /// `picture`, as the processor reads it.
        from: ID3D11VideoProcessorInputView,
        /// `pixels`, as the processor writes them.
        onto: ID3D11VideoProcessorOutputView,
        /// The pixels of the picture the window drew last.
        pixels: ID3D11Texture2D,
        /// Held by the side that has the pixels: Direct3D while it writes
        /// them, OpenGL while it draws from them. Never both, as the
        /// sharing asks. The decoder never asks for it.
        turn: Mutex<()>,
    }

    // SAFETY: the device was told to protect itself from several threads
    // (`SetMultithreadProtected`), which covers what is made from it: each
    // call into it is whole. The card runs what it is asked in the order it
    // was asked, so a copy into `picture` and a reading of it never meet
    // halfway, and `pixels` are only touched by whoever holds `turn`.
    unsafe impl Send for Shared {}
    // SAFETY: as above.
    unsafe impl Sync for Shared {}

    impl Shared {
        fn turn(&self) -> MutexGuard<'_, ()> {
            self.turn.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Has the card turn the latest picture into pixels.
        fn make_pixels(&self) -> windows::core::Result<()> {
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: BOOL::from(true),
                pInputSurface: ManuallyDrop::new(Some(self.from.clone())),
                ..Default::default()
            };
            let streams = [stream];
            // OpenGL does not draw from the pixels meanwhile.
            let turn = self.turn();
            // SAFETY: one stream in, from the view of the picture into the
            // view of the pixels, both made for this processor. Sent to
            // the card at once: OpenGL draws from the pixels next.
            let made = unsafe {
                let made = self.video.VideoProcessorBlt(&self.processor, &self.onto, 0, &streams);
                self.commands.Flush();
                made
            };
            drop(turn);
            let [stream] = streams;
            drop(ManuallyDrop::into_inner(stream.pInputSurface));
            made
        }
    }

    /// The decoder's side: leaves each picture on the card, where the
    /// window finds it.
    pub struct Keeper {
        video: ID3D11VideoDevice,
        commands: ID3D11DeviceContext,
        /// What pictures of the current size go through.
        made: Option<Made>,
    }

    /// What is made once for pictures of one size.
    struct Made {
        /// The pixels to be seen, across and down.
        size: (u32, u32),
        /// The decoder's textures: across, down, and what is in them.
        laid: (u32, u32, DXGI_FORMAT),
        shared: Arc<Shared>,
    }

    /// Where the picture in `sample` is on the card: the texture, which is
    /// one of an array of them, which one of the array, and how they are
    /// laid out: pixels across and down, and what is in them.
    ///
    /// # Safety
    ///
    /// `sample` is one a decoder gave that decodes on a graphics card.
    pub(crate) unsafe fn texture_of(
        sample: &IMFSample,
    ) -> windows::core::Result<(ID3D11Texture2D, u32, (u32, u32, DXGI_FORMAT))> {
        // SAFETY: as the caller promises: the buffer of such a sample is a
        // texture of the card's.
        unsafe {
            let buffer = sample.GetBufferByIndex(0)?.cast::<IMFDXGIBuffer>()?;
            let mut texture = std::ptr::null_mut();
            buffer.GetResource(&ID3D11Texture2D::IID, &mut texture)?;
            let source = ID3D11Texture2D::from_raw(texture);
            let mut laid = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut laid);
            Ok((source, buffer.GetSubresourceIndex()?, (laid.Width, laid.Height, laid.Format)))
        }
    }

    /// The ways to ask for the texture pictures are copied into. The first
    /// the card's video processor reads from is used: as a decoder's own
    /// textures, which the processor read before pictures were copied, then
    /// as Moonlight asks for its copies, then as one more to draw into.
    const COPIES: [u32; 3] = [
        D3D11_BIND_DECODER.0 as u32,
        D3D11_BIND_SHADER_RESOURCE.0 as u32,
        (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
    ];

    impl Keeper {
        pub fn new(device: &ID3D11Device) -> windows::core::Result<Self> {
            let video = device.cast::<ID3D11VideoDevice>()?;
            // SAFETY: asking the device for the context it always has.
            let commands = unsafe { device.GetImmediateContext() }?;
            Ok(Self { video, commands, made: None })
        }

        /// Leaves the picture in `sample`, of which `shown` pixels across
        /// and down are to be seen, on the card, and returns where the
        /// window finds it. The stream is `fps` pictures a second.
        ///
        /// Nothing is waited for: the card copies the picture when it has
        /// decoded it, and the decoder has its texture back.
        pub fn keep(
            &mut self,
            device: &ID3D11Device,
            sample: &IMFSample,
            shown: (usize, usize),
            fps: u32,
        ) -> windows::core::Result<Kept> {
            let size = (u32::try_from(shown.0).unwrap_or(0), u32::try_from(shown.1).unwrap_or(0));
            // SAFETY: reading the sample the decoder just gave.
            let (source, slice, laid) = unsafe { texture_of(sample) }?;
            if self.made.as_ref().is_none_or(|made| made.size != size || made.laid != laid) {
                self.made = Some(self.make(device, size, laid, fps)?);
            }
            let Some(made) = &self.made else {
                return Err(windows::core::Error::empty());
            };
            // SAFETY: the whole of one texture of the decoder's into one
            // made as large, of the same kind, by the same device.
            unsafe {
                self.commands.CopySubresourceRegion(
                    &made.shared.picture,
                    0,
                    0,
                    0,
                    0,
                    &source,
                    slice,
                    None,
                );
            }
            made.shared.copied.fetch_add(1, Ordering::Release);
            Ok(Kept { shared: Arc::clone(&made.shared) })
        }

        /// The textures and the video processor for pictures of `size`,
        /// decoded into textures `laid` out as the decoder's are.
        fn make(
            &self,
            device: &ID3D11Device,
            size: (u32, u32),
            laid: (u32, u32, DXGI_FORMAT),
            fps: u32,
        ) -> windows::core::Result<Made> {
            let (width, height) = size;
            let pace = DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: pace,
                InputWidth: width,
                InputHeight: height,
                OutputFrameRate: pace,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let video = self.commands.cast::<ID3D11VideoContext>()?;
            // SAFETY: plain calls into the device, with what it just made.
            unsafe {
                let enumerator = self.video.CreateVideoProcessorEnumerator(&content)?;
                let processor = self.video.CreateVideoProcessor(&enumerator, 0)?;
                let (picture, from) = self.copy(device, &enumerator, laid)?;
                let format = pixels(&enumerator)?;
                let wanted = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut texture = None;
                device.CreateTexture2D(&wanted, None, Some(&mut texture))?;
                let texture = texture.ok_or_else(windows::core::Error::empty)?;
                let onto = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut target = None;
                self.video.CreateVideoProcessorOutputView(
                    &texture,
                    &enumerator,
                    &onto,
                    Some(&mut target),
                )?;
                let target = target.ok_or_else(windows::core::Error::empty)?;

                // The picture as it is: every pixel shown, none of the
                // padding below and beside them, and nothing the driver
                // would add to "improve" a film.
                let whole = RECT {
                    left: 0,
                    top: 0,
                    right: i32::try_from(width).unwrap_or(i32::MAX),
                    bottom: i32::try_from(height).unwrap_or(i32::MAX),
                };
                let context = &video;
                context.VideoProcessorSetStreamFrameFormat(
                    &processor,
                    0,
                    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                );
                context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&whole));
                context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&whole));
                context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&whole));
                context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
                // What Leto asks PCs for, as `convert.rs` has it: BT.601,
                // brightness from 16 to 235. Out come pixels from 0 to 255.
                // Said the newer way where Windows has it, which also says
                // where the colours sit between the pixels.
                if let Ok(newer) = context.cast::<ID3D11VideoContext1>() {
                    newer.VideoProcessorSetStreamColorSpace1(
                        &processor,
                        0,
                        DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
                    );
                    newer.VideoProcessorSetOutputColorSpace1(
                        &processor,
                        DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                    );
                } else {
                    context.VideoProcessorSetStreamColorSpace(&processor, 0, &STREAM_COLOURS);
                    context.VideoProcessorSetOutputColorSpace(&processor, &PIXEL_COLOURS);
                }

                let shared = Arc::new(Shared {
                    device: device.clone(),
                    commands: self.commands.clone(),
                    video,
                    picture,
                    copied: AtomicU64::new(0),
                    processor,
                    _enumerator: enumerator,
                    from,
                    onto: target,
                    pixels: texture,
                    turn: Mutex::new(()),
                });
                Ok(Made { size, laid, shared })
            }
        }

        /// The texture pictures are copied into, as large as the decoder's
        /// and of their kind, and how the video processor reads it.
        ///
        /// # Safety
        ///
        /// Plain calls into the device the enumerator is of.
        unsafe fn copy(
            &self,
            device: &ID3D11Device,
            enumerator: &ID3D11VideoProcessorEnumerator,
            laid: (u32, u32, DXGI_FORMAT),
        ) -> windows::core::Result<(ID3D11Texture2D, ID3D11VideoProcessorInputView)> {
            let read = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut refused = windows::core::Error::empty();
            for asked in COPIES {
                let wanted = D3D11_TEXTURE2D_DESC {
                    Width: laid.0,
                    Height: laid.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: laid.2,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: asked,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                // SAFETY: as the caller promises, with what was just made.
                let made = unsafe {
                    let mut texture = None;
                    device.CreateTexture2D(&wanted, None, Some(&mut texture)).and_then(|()| {
                        let texture = texture.ok_or_else(windows::core::Error::empty)?;
                        let mut from = None;
                        self.video.CreateVideoProcessorInputView(
                            &texture,
                            enumerator,
                            &read,
                            Some(&mut from),
                        )?;
                        Ok((texture, from.ok_or_else(windows::core::Error::empty)?))
                    })
                };
                match made {
                    Ok(made) => {
                        log::debug!(
                            "pictures are copied on the graphics card into a texture of {} by \
                             {}, asked for as {asked:#x}",
                            laid.0,
                            laid.1
                        );
                        return Ok(made);
                    }
                    Err(error) => refused = error,
                }
            }
            Err(refused)
        }
    }

    /// The colours of a stream, in the bits Direct3D reads them from: for
    /// playback, BT.601 (bit 2 clear), in limited range (the two bits from
    /// bit 4 say 1, "16 to 235").
    const STREAM_COLOURS: D3D11_VIDEO_PROCESSOR_COLOR_SPACE =
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 1 << 4 };
    /// The colours of the pixels made: full range (bit 1 clear).
    const PIXEL_COLOURS: D3D11_VIDEO_PROCESSOR_COLOR_SPACE =
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };

    /// The kind of pixels the video processor makes, of the two OpenGL
    /// shares.
    ///
    /// # Safety
    ///
    /// Plain calls into the enumerator.
    unsafe fn pixels(
        enumerator: &ID3D11VideoProcessorEnumerator,
    ) -> windows::core::Result<DXGI_FORMAT> {
        for format in [DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM] {
            // SAFETY: as the caller promises.
            let support = unsafe { enumerator.CheckVideoProcessorFormat(format) }.unwrap_or(0);
            if support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 != 0 {
                return Ok(format);
            }
        }
        Err(windows::core::Error::empty())
    }

    /// What `WGL_NV_DX_interop2` gives OpenGL to share textures of Direct3D
    /// with, found in the driver.
    #[derive(Clone, Copy)]
    struct Sharing {
        open: unsafe extern "system" fn(*mut c_void) -> HANDLE,
        close: unsafe extern "system" fn(HANDLE) -> BOOL,
        register: unsafe extern "system" fn(HANDLE, *mut c_void, u32, u32, u32) -> HANDLE,
        unregister: unsafe extern "system" fn(HANDLE, HANDLE) -> BOOL,
        lock: unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> BOOL,
        unlock: unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> BOOL,
    }

    /// OpenGL only reads the texture.
    const READ_ONLY: u32 = 0;

    /// A function of the driver's, before it is known which.
    type Found = unsafe extern "system" fn() -> isize;

    impl Sharing {
        /// Whether the driver says it shares textures with Direct3D 11.
        /// Asked before any of its functions is looked for: a driver may
        /// answer for a function it has not with something that is not one.
        ///
        /// # Safety
        ///
        /// An OpenGL context is current on this thread.
        unsafe fn offered() -> bool {
            type Extensions = unsafe extern "system" fn(HDC) -> *const c_char;
            // SAFETY: asking the driver for its list of extensions, with a
            // context current as the caller promises: text that ends with
            // a zero and stays for as long as the context does.
            unsafe {
                let Some(found) = wglGetProcAddress(s!("wglGetExtensionsStringARB")) else {
                    return false;
                };
                let extensions = std::mem::transmute::<Found, Extensions>(found);
                let said = extensions(wglGetCurrentDC());
                !said.is_null()
                    && CStr::from_ptr(said)
                        .to_bytes()
                        .split(|byte| *byte == b' ')
                        .any(|name| name == b"WGL_NV_DX_interop2")
            }
        }

        /// Finds the functions, with an OpenGL context current on this
        /// thread. `None` where the driver has not all of them.
        ///
        /// # Safety
        ///
        /// An OpenGL context is current on this thread.
        unsafe fn find() -> Option<Self> {
            // SAFETY: as the caller promises.
            if !unsafe { Self::offered() } {
                return None;
            }
            // SAFETY: asking the driver for functions by name, with a
            // context current as the caller promises. Each is then called
            // with the arguments its extension gives it.
            unsafe {
                let find = |name: PCSTR| wglGetProcAddress(name);
                Some(Self {
                    open: std::mem::transmute::<
                        Found,
                        unsafe extern "system" fn(*mut c_void) -> HANDLE,
                    >(find(s!("wglDXOpenDeviceNV"))?),
                    close: std::mem::transmute::<Found, unsafe extern "system" fn(HANDLE) -> BOOL>(
                        find(s!("wglDXCloseDeviceNV"))?,
                    ),
                    register: std::mem::transmute::<
                        Found,
                        unsafe extern "system" fn(HANDLE, *mut c_void, u32, u32, u32) -> HANDLE,
                    >(find(s!("wglDXRegisterObjectNV"))?),
                    unregister: std::mem::transmute::<
                        Found,
                        unsafe extern "system" fn(HANDLE, HANDLE) -> BOOL,
                    >(find(s!("wglDXUnregisterObjectNV"))?),
                    lock: std::mem::transmute::<
                        Found,
                        unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> BOOL,
                    >(find(s!("wglDXLockObjectsNV"))?),
                    unlock: std::mem::transmute::<
                        Found,
                        unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> BOOL,
                    >(find(s!("wglDXUnlockObjectsNV"))?),
                })
            }
        }
    }

    /// Whether the OpenGL context current on this thread can draw from
    /// textures of Direct3D.
    ///
    /// # Safety
    ///
    /// An OpenGL context is current on this thread.
    pub unsafe fn shares() -> bool {
        // SAFETY: as the caller promises.
        unsafe { Sharing::find() }.is_some()
    }

    /// The window's side: the shared texture as OpenGL sees it.
    pub struct Seen {
        sharing: Sharing,
        /// The card of Direct3D, opened for OpenGL.
        device: HANDLE,
        /// The texture of Direct3D, registered as `texture`.
        object: HANDLE,
        texture: Texture,
        shared: Arc<Shared>,
        /// How the texture is read was said, at the first drawing.
        told: bool,
        /// Which picture the pixels are of: how many had been left on the
        /// card when they were made.
        taken: Option<u64>,
    }

    // SAFETY: the handles are the driver's names for things, used with the
    // OpenGL context they were made with: `Card`, which has this, is only
    // ever used on the thread that context is current on.
    unsafe impl Send for Seen {}

    impl Seen {
        /// Gives OpenGL the texture `kept` is on.
        ///
        /// # Safety
        ///
        /// `gl` is current on this thread.
        pub unsafe fn open(gl: &Gl, kept: &Kept) -> Result<Self, String> {
            let shared = Arc::clone(&kept.shared);
            // SAFETY: as the caller promises, with the functions the driver
            // gave for exactly this, and objects that live as long as
            // `shared` does, which this keeps.
            unsafe {
                let sharing = Sharing::find()
                    .ok_or("the graphics driver does not share pictures with Direct3D")?;
                let device = (sharing.open)(shared.device.as_raw());
                if device.is_invalid() || device.0.is_null() {
                    return Err(format!("OpenGL did not open the card: {:?}", GetLastError()));
                }
                let texture = match gl.create_texture() {
                    Ok(texture) => texture,
                    Err(error) => {
                        let _ = (sharing.close)(device);
                        return Err(error);
                    }
                };
                let object = (sharing.register)(
                    device,
                    shared.pixels.as_raw(),
                    texture.0.get(),
                    glow::TEXTURE_2D,
                    READ_ONLY,
                );
                if object.is_invalid() || object.0.is_null() {
                    let why = GetLastError();
                    gl.delete_texture(texture);
                    let _ = (sharing.close)(device);
                    return Err(format!("OpenGL did not take the texture: {why:?}"));
                }
                Ok(Self { sharing, device, object, texture, shared, told: false, taken: None })
            }
        }

        /// Whether this is the texture `kept` is on.
        pub fn shows(&self, kept: &Kept) -> bool {
            Arc::ptr_eq(&self.shared, &kept.shared)
        }

        /// Has the card make the pixels of the latest picture the decoder
        /// left on it, unless they are that picture's already. They are
        /// what [`Self::draw`] draws from.
        pub fn take(&mut self) -> Result<(), String> {
            let copied = self.shared.copied.load(Ordering::Acquire);
            if self.taken != Some(copied) {
                self.shared.make_pixels().map_err(|error| {
                    format!("the card made no pixels of the picture: {}", error.message())
                })?;
                self.taken = Some(copied);
            }
            Ok(())
        }

        /// Lets `draw` draw from the texture, which holds the latest
        /// picture, while Direct3D leaves it alone.
        ///
        /// # Safety
        ///
        /// `gl` is current on this thread, and is the context this was
        /// opened with.
        pub unsafe fn draw(&mut self, gl: &Gl, draw: impl FnOnce(Texture)) -> Result<(), String> {
            let _turn = self.shared.turn();
            let mut objects = [self.object];
            // SAFETY: as the caller promises, with the handles opened for
            // this context. The texture is OpenGL's between lock and unlock.
            unsafe {
                if !(self.sharing.lock)(self.device, 1, objects.as_mut_ptr()).as_bool() {
                    return Err(format!("OpenGL did not get the texture: {:?}", GetLastError()));
                }
                if !self.told {
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
                    for (what, how) in [
                        (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                        (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                    ] {
                        gl.tex_parameter_i32(glow::TEXTURE_2D, what, how as i32);
                    }
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    self.told = true;
                }
                draw(self.texture);
                // What was drawn is on its way before Direct3D writes the
                // next picture.
                gl.flush();
                let _ = (self.sharing.unlock)(self.device, 1, objects.as_mut_ptr());
            }
            Ok(())
        }

        /// Takes the texture away from OpenGL again.
        ///
        /// # Safety
        ///
        /// `gl` is current on this thread, and is the context this was
        /// opened with.
        pub unsafe fn close(self, gl: &Gl) {
            // SAFETY: as the caller promises: each handle given back once.
            unsafe {
                let _ = (self.sharing.unregister)(self.device, self.object);
                gl.delete_texture(self.texture);
                let _ = (self.sharing.close)(self.device);
            }
        }
    }
}

#[cfg(target_os = "android")]
pub(crate) mod android {
    use std::sync::{Arc, Mutex, PoisonError};

    use glow::HasContext as _;
    use leto_system::{
        decoder::{Image, Reader},
        egl,
    };

    use super::Kept;

    /// Where OpenGL ES binds a picture from elsewhere.
    pub use leto_system::egl::TEXTURE_EXTERNAL;

    type Gl = glow::Context;
    type Texture = <Gl as glow::HasContext>::Texture;

    /// What the decoder and the window share: the reader the decoder gives
    /// its pictures onto, and the newest one, waiting to be drawn.
    pub struct Shared {
        /// Dropped before the reader, which it goes back to.
        latest: Mutex<Option<Image>>,
        pub(crate) reader: Reader,
    }

    impl Shared {
        pub fn new(width: u32, height: u32) -> Result<Self, String> {
            Ok(Self { latest: Mutex::new(None), reader: Reader::new(width, height)? })
        }

        /// Takes the newest picture the decoder left on the reader, if one
        /// came: it is what the window draws next. Returns whether one did.
        pub fn take_from_reader(&self) -> bool {
            let Some(image) = self.reader.latest() else { return false };
            *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(image);
            true
        }
    }

    /// The window's side: the picture being drawn, as a texture.
    pub struct Seen {
        texture: Texture,
        /// The picture drawn, its texture, and where it is in its buffer.
        /// The texture goes before the picture it is made of.
        current: Option<(egl::Texture, Image, [f32; 4])>,
        /// Last, so the reader outlives its pictures.
        shared: Arc<Shared>,
    }

    // SAFETY: the texture is the context's name for it, used only on the
    // thread that context is current on, where `Card`, which has this, is.
    unsafe impl Send for Seen {}

    impl Seen {
        /// Gives OpenGL a texture for the pictures `kept` stands for.
        ///
        /// # Safety
        ///
        /// `gl` is current on this thread.
        pub unsafe fn open(gl: &Gl, kept: &Kept) -> Result<Self, String> {
            // SAFETY: as the caller promises.
            let texture = unsafe { gl.create_texture()? };
            // SAFETY: as above, with the texture just made.
            unsafe {
                gl.bind_texture(egl::TEXTURE_EXTERNAL, Some(texture));
                for (what, how) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(egl::TEXTURE_EXTERNAL, what, how as i32);
                }
                gl.bind_texture(egl::TEXTURE_EXTERNAL, None);
            }
            Ok(Self { texture, current: None, shared: Arc::clone(&kept.shared) })
        }

        /// Whether this draws the pictures `kept` stands for.
        pub fn shows(&self, kept: &Kept) -> bool {
            Arc::ptr_eq(&self.shared, &kept.shared)
        }

        /// Makes the newest picture the decoder left the texture, unless
        /// none came since: it is what [`Self::draw`] draws from.
        ///
        /// # Safety
        ///
        /// `gl` is current on this thread, and is the one this was opened
        /// with.
        pub unsafe fn take(&mut self, gl: &Gl) -> Result<(), String> {
            let newest = self.shared.latest.lock().unwrap_or_else(PoisonError::into_inner).take();
            let Some(image) = newest else { return Ok(()) };
            let place = image.place();
            // SAFETY: as the caller promises; the picture lives as long as
            // its texture, which goes first.
            unsafe {
                gl.bind_texture(egl::TEXTURE_EXTERNAL, Some(self.texture));
                let bound = egl::Texture::bind(&image);
                gl.bind_texture(egl::TEXTURE_EXTERNAL, None);
                // The picture before goes back to the reader.
                self.current = Some((bound?, image, place));
            }
            Ok(())
        }

        /// The texture to draw from and where the picture is in it, once a
        /// picture came.
        pub fn drawn(&self) -> Option<(Texture, [f32; 4])> {
            self.current.as_ref().map(|(_, _, place)| (self.texture, *place))
        }

        /// Gives the texture back, and the picture to the reader.
        ///
        /// # Safety
        ///
        /// `gl` is current on this thread, and is the one this was opened
        /// with.
        pub unsafe fn close(mut self, gl: &Gl) {
            self.current = None;
            // SAFETY: as the caller promises, deleted once.
            unsafe { gl.delete_texture(self.texture) };
        }
    }

    /// Whether the window's OpenGL ES, current on this thread, draws the
    /// decoder's pictures from where they are.
    pub fn shares(gl: &Gl) -> bool {
        gl.version().is_embedded
            && gl.supported_extensions().contains("GL_OES_EGL_image_external_essl3")
            && egl::takes_pictures()
    }
}
