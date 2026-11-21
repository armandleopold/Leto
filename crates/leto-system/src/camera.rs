//! The cameras of an Android phone, read through the NDK's Camera2: the
//! cameras of other systems are read by `leto-media` itself.
//!
//! A picture comes as the camera made it, in planes of YUV 4:2:0, with how
//! far it must be turned to stand upright on the screen as it is held now.

use std::{
    ffi::{CStr, CString, c_int, c_void},
    ptr,
    time::{Duration, Instant},
};

use ndk_sys as sys;

#[link(name = "camera2ndk")]
unsafe extern "C" {}

/// The largest picture asked of a camera: what a PC is sent is not larger.
const LARGEST: (i32, i32) = (1280, 720);
/// How long a picture is waited for before the camera is taken as gone.
const PATIENCE: Duration = Duration::from_secs(3);

/// One picture, in the camera's planes.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u8>,
    pub luma_stride: usize,
    /// U and V, each with a row stride and the distance between samples:
    /// 2 when they are interleaved, as most phones do.
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub chroma_stride: usize,
    pub chroma_step: usize,
    /// Clockwise, in degrees: 0, 90, 180 or 270.
    pub turn: u32,
}

/// The cameras of the phone by the names the person sees, the front one
/// first: it is the one that looks at them.
pub fn names() -> Result<Vec<String>, String> {
    let manager = Manager::new()?;
    let mut found = manager.cameras()?;
    found.sort_by_key(|camera| camera.facing.rank());
    let mut names = Vec::new();
    for camera in found {
        let name = camera.name();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

/// A camera, capturing from when it is opened until it is dropped.
pub struct Camera {
    // Dropped in this order: what captures first, the manager last.
    session: *mut sys::ACameraCaptureSession,
    request: *mut sys::ACaptureRequest,
    target: *mut sys::ACameraOutputTarget,
    container: *mut sys::ACaptureSessionOutputContainer,
    output: *mut sys::ACaptureSessionOutput,
    device: *mut sys::ACameraDevice,
    reader: *mut sys::AImageReader,
    callbacks: Box<sys::ACameraDevice_StateCallbacks>,
    session_callbacks: Box<sys::ACameraCaptureSession_stateCallbacks>,
    manager: Manager,
    /// The camera's own turn, and which way it faces.
    sensor: u32,
    facing: Facing,
    pub name: String,
    pub width: usize,
    pub height: usize,
}

// SAFETY: Camera2's objects may be used from any thread; Leto uses one
// camera from one thread at a time.
unsafe impl Send for Camera {}

impl Camera {
    /// Opens the camera called `name`, or the front one, or the first.
    pub fn open(name: Option<&str>) -> Result<Self, String> {
        let manager = Manager::new()?;
        let mut cameras = manager.cameras()?;
        cameras.sort_by_key(|camera| camera.facing.rank());
        let chosen = name
            .and_then(|name| cameras.iter().position(|camera| camera.name() == name))
            .unwrap_or(0);
        if cameras.is_empty() {
            return Err("this device has no camera".to_owned());
        }
        let info = cameras.swap_remove(chosen);
        let (width, height) = info.size;

        let mut camera = Self {
            session: ptr::null_mut(),
            request: ptr::null_mut(),
            target: ptr::null_mut(),
            container: ptr::null_mut(),
            output: ptr::null_mut(),
            device: ptr::null_mut(),
            reader: ptr::null_mut(),
            callbacks: Box::new(sys::ACameraDevice_StateCallbacks {
                context: ptr::null_mut(),
                onDisconnected: Some(disconnected),
                onError: Some(failed),
            }),
            session_callbacks: Box::new(sys::ACameraCaptureSession_stateCallbacks {
                context: ptr::null_mut(),
                onClosed: Some(quiet),
                onReady: Some(quiet),
                onActive: Some(quiet),
            }),
            manager,
            sensor: info.sensor,
            facing: info.facing,
            name: info.name(),
            width: width as usize,
            height: height as usize,
        };
        // SAFETY: each call gets live objects made above, and what it makes
        // is kept in `camera`, whose drop frees it.
        unsafe {
            media(
                sys::AImageReader_new(
                    width,
                    height,
                    sys::AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32,
                    4,
                    &mut camera.reader,
                ),
                "no picture reader",
            )?;
            let mut window = ptr::null_mut();
            media(sys::AImageReader_getWindow(camera.reader, &mut window), "no window")?;
            let id = CString::new(info.id).map_err(|_| "a camera with a strange id")?;
            status(
                sys::ACameraManager_openCamera(
                    camera.manager.0,
                    id.as_ptr(),
                    &mut *camera.callbacks,
                    &mut camera.device,
                ),
                "it could not be opened",
            )?;
            status(sys::ACaptureSessionOutput_create(window, &mut camera.output), "no output")?;
            status(
                sys::ACaptureSessionOutputContainer_create(&mut camera.container),
                "no outputs",
            )?;
            status(
                sys::ACaptureSessionOutputContainer_add(camera.container, camera.output),
                "outputs",
            )?;
            status(sys::ACameraOutputTarget_create(window, &mut camera.target), "no target")?;
            status(
                sys::ACameraDevice_createCaptureRequest(
                    camera.device,
                    sys::ACameraDevice_request_template::TEMPLATE_RECORD,
                    &mut camera.request,
                ),
                "no request",
            )?;
            status(sys::ACaptureRequest_addTarget(camera.request, camera.target), "target")?;
            status(
                sys::ACameraDevice_createCaptureSession(
                    camera.device,
                    camera.container,
                    &*camera.session_callbacks,
                    &mut camera.session,
                ),
                "no capture session",
            )?;
            let mut requests = [camera.request];
            status(
                sys::ACameraCaptureSession_setRepeatingRequest(
                    camera.session,
                    ptr::null_mut(),
                    1,
                    requests.as_mut_ptr(),
                    ptr::null_mut(),
                ),
                "it would not capture",
            )?;
        }
        log::info!(
            "the camera \"{}\" captures {width}x{height}, turned {}° on its sensor",
            camera.name,
            camera.sensor
        );
        Ok(camera)
    }

    /// Waits for the next picture.
    pub fn picture(&mut self) -> Result<Frame, String> {
        let start = Instant::now();
        loop {
            let mut image = ptr::null_mut();
            // SAFETY: the reader lives as long as `self`.
            let got = unsafe { sys::AImageReader_acquireLatestImage(self.reader, &mut image) };
            if got == sys::media_status_t::AMEDIA_OK && !image.is_null() {
                // SAFETY: the image was just acquired, and is deleted once
                // copied.
                let frame = unsafe { self.copy(image) };
                // SAFETY: as above.
                unsafe { sys::AImage_delete(image) };
                return frame;
            }
            if got != sys::media_status_t::AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE
                && got != sys::media_status_t::AMEDIA_OK
            {
                return Err(format!("it gave no picture ({})", got.0));
            }
            if start.elapsed() > PATIENCE {
                return Err("it stopped giving pictures".to_owned());
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    /// # Safety
    ///
    /// `image` is an image of this camera's reader, not yet deleted.
    unsafe fn copy(&self, image: *mut sys::AImage) -> Result<Frame, String> {
        let plane = |index: c_int| -> Result<(Vec<u8>, usize, usize), String> {
            let (mut data, mut length) = (ptr::null_mut(), 0);
            let (mut row, mut step) = (0, 0);
            // SAFETY: the caller's promise; the plane's memory is read
            // within its length while the image lives.
            unsafe {
                media(sys::AImage_getPlaneData(image, index, &mut data, &mut length), "plane")?;
                media(sys::AImage_getPlaneRowStride(image, index, &mut row), "plane")?;
                media(sys::AImage_getPlanePixelStride(image, index, &mut step), "plane")?;
                let bytes = std::slice::from_raw_parts(data, length.max(0) as usize).to_vec();
                Ok((bytes, row.max(0) as usize, step.max(1) as usize))
            }
        };
        let (mut width, mut height) = (0, 0);
        // SAFETY: the caller's promise.
        unsafe {
            sys::AImage_getWidth(image, &mut width);
            sys::AImage_getHeight(image, &mut height);
        }
        let (luma, luma_stride, _) = plane(0)?;
        let (u, chroma_stride, chroma_step) = plane(1)?;
        let (v, _, _) = plane(2)?;
        Ok(Frame {
            width: width.max(0) as usize,
            height: height.max(0) as usize,
            luma,
            luma_stride,
            u,
            v,
            chroma_stride,
            chroma_step,
            turn: self.turn(),
        })
    }

    /// How far a picture must turn to stand upright as the phone is held.
    fn turn(&self) -> u32 {
        let held = crate::java::with_java(|env, activity| {
            let manager = env
                .call_method(activity, "getWindowManager", "()Landroid/view/WindowManager;", &[])?
                .l()?;
            let display = env
                .call_method(&manager, "getDefaultDisplay", "()Landroid/view/Display;", &[])?
                .l()?;
            env.call_method(&display, "getRotation", "()I", &[])?.i()
        })
        .unwrap_or(0);
        let held = (held.clamp(0, 3) as u32) * 90;
        match self.facing {
            Facing::Front => (self.sensor + held) % 360,
            Facing::Back | Facing::External => (self.sensor + 360 - held) % 360,
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: each object is freed once, after what uses it.
        unsafe {
            if !self.session.is_null() {
                sys::ACameraCaptureSession_stopRepeating(self.session);
                sys::ACameraCaptureSession_close(self.session);
            }
            if !self.request.is_null() {
                sys::ACaptureRequest_free(self.request);
            }
            if !self.target.is_null() {
                sys::ACameraOutputTarget_free(self.target);
            }
            if !self.container.is_null() {
                sys::ACaptureSessionOutputContainer_free(self.container);
            }
            if !self.output.is_null() {
                sys::ACaptureSessionOutput_free(self.output);
            }
            if !self.device.is_null() {
                sys::ACameraDevice_close(self.device);
            }
            if !self.reader.is_null() {
                sys::AImageReader_delete(self.reader);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Facing {
    Front,
    Back,
    External,
}

impl Facing {
    fn rank(self) -> u8 {
        match self {
            Self::Front => 0,
            Self::Back => 1,
            Self::External => 2,
        }
    }
}

struct Info {
    id: String,
    facing: Facing,
    sensor: u32,
    size: (i32, i32),
    /// Its place among those that face the same way.
    nth: usize,
}

impl Info {
    fn name(&self) -> String {
        let side = match self.facing {
            Facing::Front => "Front camera",
            Facing::Back => "Back camera",
            Facing::External => "External camera",
        };
        if self.nth == 0 { side.to_owned() } else { format!("{side} {}", self.nth + 1) }
    }
}

struct Manager(*mut sys::ACameraManager);

impl Manager {
    fn new() -> Result<Self, String> {
        // SAFETY: no argument; null when the system has no camera service.
        let manager = unsafe { sys::ACameraManager_create() };
        if manager.is_null() {
            return Err("the system's cameras cannot be reached".to_owned());
        }
        Ok(Self(manager))
    }

    fn cameras(&self) -> Result<Vec<Info>, String> {
        let mut list = ptr::null_mut();
        // SAFETY: the manager is alive; the list is freed below.
        unsafe { status(sys::ACameraManager_getCameraIdList(self.0, &mut list), "no list")? };
        let mut found: Vec<Info> = Vec::new();
        // SAFETY: the list holds `numCameras` C strings until it is freed.
        unsafe {
            let ids =
                std::slice::from_raw_parts((*list).cameraIds, (*list).numCameras.max(0) as usize);
            for &id in ids {
                let id = CStr::from_ptr(id).to_string_lossy().into_owned();
                match self.describe(&id) {
                    Ok(Some(mut info)) => {
                        info.nth = found.iter().filter(|other| other.facing == info.facing).count();
                        found.push(info);
                    }
                    Ok(None) => {}
                    Err(error) => log::debug!("the camera {id} is left out: {error}"),
                }
            }
            sys::ACameraManager_deleteCameraIdList(list);
        }
        Ok(found)
    }

    /// What Leto needs of a camera, or `None` when it gives no YUV.
    fn describe(&self, id: &str) -> Result<Option<Info>, String> {
        let name = CString::new(id).map_err(|_| "a strange id")?;
        let mut traits = ptr::null_mut();
        // SAFETY: the manager is alive; the metadata is freed below, and
        // entries are read within their count before that.
        unsafe {
            status(
                sys::ACameraManager_getCameraCharacteristics(self.0, name.as_ptr(), &mut traits),
                "no description",
            )?;
            let numbers = |tag: sys::acamera_metadata_tag| -> Vec<i32> {
                let mut entry = std::mem::zeroed::<sys::ACameraMetadata_const_entry>();
                if sys::ACameraMetadata_getConstEntry(traits, tag.0, &mut entry)
                    != sys::camera_status_t::ACAMERA_OK
                {
                    return Vec::new();
                }
                if tag == sys::acamera_metadata_tag::ACAMERA_LENS_FACING {
                    std::slice::from_raw_parts(entry.data.u8_, entry.count as usize)
                        .iter()
                        .map(|&byte| i32::from(byte))
                        .collect()
                } else {
                    std::slice::from_raw_parts(entry.data.i32_, entry.count as usize).to_vec()
                }
            };
            let facing = match numbers(sys::acamera_metadata_tag::ACAMERA_LENS_FACING).first() {
                Some(0) => Facing::Front,
                Some(1) => Facing::Back,
                _ => Facing::External,
            };
            let sensor = numbers(sys::acamera_metadata_tag::ACAMERA_SENSOR_ORIENTATION)
                .first()
                .map_or(0, |&turn| turn.rem_euclid(360) as u32);
            let configurations =
                numbers(sys::acamera_metadata_tag::ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS);
            sys::ACameraMetadata_free(traits);
            let yuv = sys::AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32;
            let sizes = configurations
                .chunks_exact(4)
                .filter(|four| four[0] == yuv && four[3] == 0)
                .map(|four| (four[1], four[2]));
            Ok(choose(sizes).map(|size| Info { id: id.to_owned(), facing, sensor, size, nth: 0 }))
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // SAFETY: made by `ACameraManager_create`, freed once.
        unsafe { sys::ACameraManager_delete(self.0) };
    }
}

/// The largest size within [`LARGEST`], the widest first among equals;
/// the smallest one when none fits.
fn choose(sizes: impl Iterator<Item = (i32, i32)>) -> Option<(i32, i32)> {
    let sizes: Vec<_> = sizes.filter(|(width, height)| *width > 0 && *height > 0).collect();
    let fits = |(width, height): &(i32, i32)| *width <= LARGEST.0 && *height <= LARGEST.0;
    sizes
        .iter()
        .filter(|size| fits(size) && size.0.min(size.1) <= LARGEST.1)
        .max_by_key(|(width, height)| (i64::from(*width) * i64::from(*height), *width))
        .or_else(|| {
            sizes.iter().min_by_key(|(width, height)| i64::from(*width) * i64::from(*height))
        })
        .copied()
}

fn status(status: sys::camera_status_t, what: &str) -> Result<(), String> {
    if status == sys::camera_status_t::ACAMERA_OK {
        return Ok(());
    }
    if status == sys::camera_status_t::ACAMERA_ERROR_PERMISSION_DENIED {
        return Err("Leto may not use the camera".to_owned());
    }
    Err(format!("{what} ({})", status.0))
}

fn media(status: sys::media_status_t, what: &str) -> Result<(), String> {
    if status == sys::media_status_t::AMEDIA_OK {
        Ok(())
    } else {
        Err(format!("{what} ({})", status.0))
    }
}

unsafe extern "C" fn disconnected(_: *mut c_void, _: *mut sys::ACameraDevice) {
    log::warn!("the camera was taken away");
}

unsafe extern "C" fn failed(_: *mut c_void, _: *mut sys::ACameraDevice, error: c_int) {
    log::warn!("the camera failed ({error})");
}

unsafe extern "C" fn quiet(_: *mut c_void, _: *mut sys::ACameraCaptureSession) {}
