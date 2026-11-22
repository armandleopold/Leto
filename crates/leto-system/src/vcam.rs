//! The pictures of a client's camera, handed to the virtual camera that
//! comes with OBS Studio: the apps of the PC, Teams among them, then see
//! them when they choose "OBS Virtual Camera".
//!
//! OBS's camera reads its pictures from a piece of memory with a name,
//! which OBS writes while its own virtual camera runs. Leto writes it the
//! same way (`shared-memory-queue.c` in OBS's `win-dshow` plugin, read at
//! commit 0c790bd): a header, then three pictures in NV12, each after its
//! time. Only one program writes at a time. Windows only: elsewhere
//! nothing opens.

/// Why the virtual camera does not take Leto's pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Another program writes to it: OBS, with its own virtual camera on.
    Taken,
    /// Windows did not give the memory, or this is not Windows.
    Unavailable,
}

/// The virtual camera while Leto writes to it, in one size. Letting go of
/// it tells the camera that the pictures stopped.
pub struct Feed {
    #[cfg(windows)]
    inner: windows::Feed,
    width: u32,
    height: u32,
}

impl Feed {
    /// Starts writing pictures of `width` by `height`, both even, that
    /// come every `interval` hundreds of nanoseconds.
    pub fn open(width: u32, height: u32, interval: u64) -> Result<Self, Refused> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(Refused::Unavailable);
        }
        #[cfg(windows)]
        return Ok(Self { inner: windows::Feed::open(width, height, interval)?, width, height });
        #[cfg(not(windows))]
        {
            let _ = interval;
            Err(Refused::Unavailable)
        }
    }

    /// The size it was opened in.
    pub const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Hands over a picture in NV12: `luma`, a byte a pixel, then `chroma`,
    /// two bytes for every four pixels, taken at `time`, in nanoseconds. A
    /// picture of another size is left out.
    pub fn write(&mut self, luma: &[u8], chroma: &[u8], time: u64) {
        let pixels = self.width as usize * self.height as usize;
        if luma.len() != pixels || chroma.len() != pixels / 2 {
            return;
        }
        #[cfg(windows)]
        self.inner.write(luma, chroma, time);
        #[cfg(not(windows))]
        let _ = time;
    }
}

/// Where the three pictures start in the memory, and how large it is, for
/// pictures of `frame` bytes after a header of `header` bytes: each
/// picture comes after 32 bytes for its time, and all start on a multiple
/// of 32.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
const fn layout(header: usize, frame: usize) -> ([usize; 3], usize) {
    const fn aligned(size: usize) -> usize {
        (size + 31) & !31
    }
    let first = aligned(header);
    let second = aligned(first + frame + TIME);
    let third = aligned(second + frame + TIME);
    ([first, second, third], aligned(third + frame + TIME))
}

/// The room before each picture, where its time is.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
const TIME: usize = 32;

#[cfg(windows)]
mod windows {
    use std::{mem, ptr};

    use windows_sys::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
            Security::{
                Authorization::{
                    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
                },
                PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
            },
            System::Memory::{
                CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS,
                MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
            },
        },
        core::w,
    };

    use super::{Refused, TIME, layout};

    /// What OBS's camera reads first, as OBS lays it out.
    #[repr(C)]
    struct Header {
        write_idx: u32,
        read_idx: u32,
        state: u32,
        offsets: [u32; 3],
        kind: u32,
        cx: u32,
        cy: u32,
        interval: u64,
        reserved: [u32; 8],
    }

    const STARTING: u32 = 1;
    const READY: u32 = 2;
    const STOPPING: u32 = 3;

    pub struct Feed {
        handle: HANDLE,
        header: *mut Header,
        offsets: [usize; 3],
    }

    // SAFETY: the memory is this feed's own view, written through `&mut`.
    unsafe impl Send for Feed {}

    impl Feed {
        pub fn open(width: u32, height: u32, interval: u64) -> Result<Self, Refused> {
            let frame = width as usize * height as usize * 3 / 2;
            let (offsets, size) = layout(mem::size_of::<Header>(), frame);
            let size = u32::try_from(size).map_err(|_| Refused::Unavailable)?;
            let name = w!("OBSVirtualCamVideo");
            // SAFETY: the name ends with a zero, the security descriptor
            // lives until the mapping is made, and the view is as large as
            // the mapping, which holds the header.
            unsafe {
                // Whoever writes already keeps the name: OBS, or another
                // Leto.
                let taken = OpenFileMappingW(FILE_MAP_READ, 0, name);
                if !taken.is_null() {
                    CloseHandle(taken);
                    return Err(Refused::Taken);
                }
                // A Leto started as administrator still lets the apps of
                // the person read: they are not administrators.
                let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
                let readable = w!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)(A;;GR;;;WD)(A;;GR;;;AC)");
                let described = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    readable,
                    SDDL_REVISION_1,
                    &mut descriptor,
                    ptr::null_mut(),
                ) != 0;
                let attributes = SECURITY_ATTRIBUTES {
                    nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                };
                let handle = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    if described { &attributes } else { ptr::null() },
                    PAGE_READWRITE,
                    0,
                    size,
                    name,
                );
                if described {
                    LocalFree(descriptor.cast());
                }
                if handle.is_null() {
                    return Err(Refused::Unavailable);
                }
                let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
                if view.Value.is_null() {
                    CloseHandle(handle);
                    return Err(Refused::Unavailable);
                }
                let header = view.Value.cast::<Header>();
                header.write(Header {
                    write_idx: 0,
                    read_idx: 0,
                    state: STARTING,
                    // The memory is far smaller than four gigabytes: `size`
                    // fits, and so do these.
                    offsets: offsets.map(|offset| offset as u32),
                    kind: 0,
                    cx: width,
                    cy: height,
                    interval,
                    reserved: [0; 8],
                });
                Ok(Self { handle, header, offsets })
            }
        }

        pub fn write(&mut self, luma: &[u8], chroma: &[u8], time: u64) {
            // SAFETY: `super::Feed::write` lets through only a picture of
            // the size the memory was made for, and each of the three
            // places holds its time and one such picture.
            unsafe {
                let header = self.header;
                let count = (&raw const (*header).write_idx).read_volatile().wrapping_add(1);
                (&raw mut (*header).write_idx).write_volatile(count);
                let place = self.header.cast::<u8>().add(self.offsets[count as usize % 3]);
                place.cast::<u64>().write_unaligned(time);
                let picture = place.add(TIME);
                ptr::copy_nonoverlapping(luma.as_ptr(), picture, luma.len());
                ptr::copy_nonoverlapping(chroma.as_ptr(), picture.add(luma.len()), chroma.len());
                (&raw mut (*header).read_idx).write_volatile(count);
                (&raw mut (*header).state).write_volatile(READY);
            }
        }
    }

    impl Drop for Feed {
        fn drop(&mut self) {
            // SAFETY: the view and the handle are this feed's, until here.
            unsafe {
                (&raw mut (*self.header).state).write_volatile(STOPPING);
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.header.cast() });
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pictures_sit_where_obs_looks_for_them() {
        // OBS's header is 80 bytes; a picture of 1280 by 720 in NV12 is
        // 1 382 400 bytes.
        let (offsets, size) = layout(80, 1280 * 720 * 3 / 2);
        assert_eq!(offsets, [96, 1_382_528, 2_764_960]);
        assert_eq!(size, 4_147_392);
        // A size whose pictures do not end on a multiple of 32.
        let (offsets, size) = layout(80, 6 * 2 * 3 / 2);
        assert_eq!(offsets, [96, 160, 224]);
        assert_eq!(size, 288);
    }

    #[test]
    fn a_size_that_is_not_even_is_refused() {
        assert!(matches!(Feed::open(641, 360, 333_333), Err(Refused::Unavailable)));
        assert!(matches!(Feed::open(0, 0, 333_333), Err(Refused::Unavailable)));
    }
}
