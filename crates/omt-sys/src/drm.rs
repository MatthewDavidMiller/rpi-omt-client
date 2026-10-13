//! The slice of the Linux DRM kernel ABI the receiver uses: legacy and
//! atomic mode-setting, planes and their properties, dumb buffers, and
//! page-flip events. Declared here rather than taken from <drm/drm.h>,
//! which Alpine's linux-headers does not ship. The layouts are the kernel's
//! stable ABI; every size is asserted below, and the ABI probe compares
//! structs and ioctl numbers with the real header where one is installed.

#![allow(missing_docs)]

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

/// The generic Linux ioctl encoding, which arm64 and x86-64 both use.
const fn iowr(nr: u32, size: usize) -> u32 {
    ((IOC_READ | IOC_WRITE) << 30) | ((size as u32) << 16) | ((b'd' as u32) << 8) | nr
}

const fn iow(nr: u32, size: usize) -> u32 {
    (IOC_WRITE << 30) | ((size as u32) << 16) | ((b'd' as u32) << 8) | nr
}

pub const DRM_CAP_DUMB_BUFFER: u64 = 0x1;
pub const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;
pub const DRM_MODE_FLAG_INTERLACE: u32 = 1 << 4;
pub const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
pub const DRM_MODE_CONNECTED: u32 = 1;
pub const DRM_CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
pub const DRM_CLIENT_CAP_ATOMIC: u64 = 3;
pub const DRM_MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;
pub const DRM_MODE_ATOMIC_NONBLOCK: u32 = 0x0200;
pub const DRM_MODE_ATOMIC_ALLOW_MODESET: u32 = 0x0400;
pub const DRM_MODE_OBJECT_CRTC: u32 = 0xcccc_cccc;
pub const DRM_MODE_OBJECT_CONNECTOR: u32 = 0xc0c0_c0c0;
pub const DRM_MODE_OBJECT_PLANE: u32 = 0xeeee_eeee;
pub const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
pub const DRM_PLANE_TYPE_PRIMARY: u64 = 1;
/// fourcc_code('Y', 'U', '1', '6'): three-plane YCbCr 4:2:2.
pub const DRM_FORMAT_YUV422: u32 = 0x3631_5559;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_set_client_cap {
    pub capability: u64,
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_card_res {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_get_plane_res {
    pub plane_id_ptr: u64,
    pub count_planes: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_get_plane {
    pub plane_id: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub possible_crtcs: u32,
    pub gamma_size: u32,
    pub count_format_types: u32,
    pub format_type_ptr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_obj_get_properties {
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_props: u32,
    pub obj_id: u32,
    pub obj_type: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_property_enum {
    pub value: u64,
    pub name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_get_property {
    pub values_ptr: u64,
    pub enum_blob_ptr: u64,
    pub prop_id: u32,
    pub flags: u32,
    pub name: [u8; 32],
    pub count_values: u32,
    pub count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_create_blob {
    pub data: u64,
    pub length: u32,
    pub blob_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_destroy_blob {
    pub blob_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_atomic {
    pub flags: u32,
    pub count_objs: u32,
    pub objs_ptr: u64,
    pub count_props_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub reserved: u64,
    pub user_data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_fb_cmd2 {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: u32,
    pub flags: u32,
    pub handles: [u32; 4],
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub pad: u32,
    pub modifier: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_get_cap {
    pub capability: u64,
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_modeinfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub kind: u32,
    pub name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_get_connector {
    pub encoders_ptr: u64,
    pub modes_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_modes: u32,
    pub count_props: u32,
    pub count_encoders: u32,
    pub encoder_id: u32,
    pub connector_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_get_encoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_crtc {
    pub set_connectors_ptr: u64,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub gamma_size: u32,
    pub mode_valid: u32,
    pub mode: drm_mode_modeinfo,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_fb_cmd {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
    pub depth: u32,
    pub handle: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_crtc_page_flip {
    pub crtc_id: u32,
    pub fb_id: u32,
    pub flags: u32,
    pub reserved: u32,
    pub user_data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_create_dumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_map_dumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_mode_destroy_dumb {
    pub handle: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_event {
    pub kind: u32,
    pub length: u32,
}

const _: () = {
    assert!(size_of::<drm_get_cap>() == 16);
    assert!(size_of::<drm_mode_modeinfo>() == 68);
    assert!(size_of::<drm_mode_get_connector>() == 80);
    assert!(size_of::<drm_mode_get_encoder>() == 20);
    assert!(size_of::<drm_mode_crtc>() == 104);
    assert!(size_of::<drm_mode_fb_cmd>() == 28);
    assert!(size_of::<drm_mode_crtc_page_flip>() == 24);
    assert!(size_of::<drm_mode_create_dumb>() == 32);
    assert!(size_of::<drm_mode_map_dumb>() == 16);
    assert!(size_of::<drm_set_client_cap>() == 16);
    assert!(size_of::<drm_mode_card_res>() == 64);
    assert!(size_of::<drm_mode_get_plane_res>() == 16);
    assert!(size_of::<drm_mode_get_plane>() == 32);
    assert!(size_of::<drm_mode_obj_get_properties>() == 32);
    assert!(size_of::<drm_mode_property_enum>() == 40);
    assert!(size_of::<drm_mode_get_property>() == 64);
    assert!(size_of::<drm_mode_create_blob>() == 16);
    assert!(size_of::<drm_mode_destroy_blob>() == 4);
    assert!(size_of::<drm_mode_atomic>() == 56);
    assert!(size_of::<drm_mode_fb_cmd2>() == 104);
    assert!(size_of::<drm_event>() == 8);
};

pub const DRM_IOCTL_GET_CAP: Request<drm_get_cap> =
    Request::new(iowr(0x0c, size_of::<drm_get_cap>()));
pub const DRM_IOCTL_SET_CLIENT_CAP: Request<drm_set_client_cap> =
    Request::new(iow(0x0d, size_of::<drm_set_client_cap>()));
pub const DRM_IOCTL_MODE_GETRESOURCES: u32 = iowr(0xA0, size_of::<drm_mode_card_res>());
pub const DRM_IOCTL_MODE_SETCRTC: u32 = iowr(0xA2, size_of::<drm_mode_crtc>());
pub const DRM_IOCTL_MODE_GETENCODER: Request<drm_mode_get_encoder> =
    Request::new(iowr(0xA6, size_of::<drm_mode_get_encoder>()));
pub const DRM_IOCTL_MODE_GETCONNECTOR: u32 = iowr(0xA7, size_of::<drm_mode_get_connector>());
pub const DRM_IOCTL_MODE_GETPROPERTY: u32 = iowr(0xAA, size_of::<drm_mode_get_property>());
pub const DRM_IOCTL_MODE_ADDFB: Request<drm_mode_fb_cmd> =
    Request::new(iowr(0xAE, size_of::<drm_mode_fb_cmd>()));
pub const DRM_IOCTL_MODE_RMFB: Request<u32> = Request::new(iowr(0xAF, size_of::<u32>()));
pub const DRM_IOCTL_MODE_PAGE_FLIP: Request<drm_mode_crtc_page_flip> =
    Request::new(iowr(0xB0, size_of::<drm_mode_crtc_page_flip>()));
pub const DRM_IOCTL_MODE_CREATE_DUMB: Request<drm_mode_create_dumb> =
    Request::new(iowr(0xB2, size_of::<drm_mode_create_dumb>()));
pub const DRM_IOCTL_MODE_MAP_DUMB: Request<drm_mode_map_dumb> =
    Request::new(iowr(0xB3, size_of::<drm_mode_map_dumb>()));
pub const DRM_IOCTL_MODE_DESTROY_DUMB: Request<drm_mode_destroy_dumb> =
    Request::new(iowr(0xB4, size_of::<drm_mode_destroy_dumb>()));
pub const DRM_IOCTL_MODE_GETPLANERESOURCES: u32 = iowr(0xB5, size_of::<drm_mode_get_plane_res>());
pub const DRM_IOCTL_MODE_GETPLANE: u32 = iowr(0xB6, size_of::<drm_mode_get_plane>());
pub const DRM_IOCTL_MODE_ADDFB2: Request<drm_mode_fb_cmd2> =
    Request::new(iowr(0xB8, size_of::<drm_mode_fb_cmd2>()));
pub const DRM_IOCTL_MODE_OBJ_GETPROPERTIES: u32 =
    iowr(0xB9, size_of::<drm_mode_obj_get_properties>());
pub const DRM_IOCTL_MODE_ATOMIC: u32 = iowr(0xBC, size_of::<drm_mode_atomic>());
pub const DRM_IOCTL_MODE_CREATEPROPBLOB: u32 = iowr(0xBD, size_of::<drm_mode_create_blob>());
pub const DRM_IOCTL_MODE_DESTROYPROPBLOB: Request<drm_mode_destroy_blob> =
    Request::new(iowr(0xBE, size_of::<drm_mode_destroy_blob>()));

/// A request number tied to the structure it carries: its size is encoded
/// in the number, so a request can only ever be issued on that structure.
#[derive(Clone, Copy, Debug)]
pub struct Request<T>(u32, std::marker::PhantomData<T>);

impl<T> Request<T> {
    const fn new(number: u32) -> Self {
        Self(number, std::marker::PhantomData)
    }

    pub const fn number(self) -> u32 {
        self.0
    }
}

/// A request structure the kernel reads and writes in place, with no field
/// it follows as a user-space pointer. Only these can go through the safe
/// [`ioctl`]; the rest have wrappers in [`safe`].
pub trait SelfContained: Copy {}

impl SelfContained for drm_get_cap {}
impl SelfContained for drm_set_client_cap {}
impl SelfContained for drm_mode_get_encoder {}
impl SelfContained for drm_mode_fb_cmd {}
impl SelfContained for drm_mode_fb_cmd2 {}
impl SelfContained for drm_mode_crtc_page_flip {}
impl SelfContained for drm_mode_create_dumb {}
impl SelfContained for drm_mode_map_dumb {}
impl SelfContained for drm_mode_destroy_dumb {}
impl SelfContained for drm_mode_destroy_blob {}
impl SelfContained for u32 {}

/// Issues one DRM ioctl on a self-contained structure, retrying on EINTR and
/// EAGAIN.
pub fn ioctl<T: SelfContained>(
    fd: std::ffi::c_int,
    request: Request<T>,
    arg: &mut T,
) -> std::io::Result<()> {
    // SAFETY: `arg` is a valid, exclusively borrowed `T` for the duration of
    // the call; the request was built from size_of::<T>(), so the kernel
    // copies exactly a `T`, and a SelfContained structure has no pointer for
    // the kernel to follow.
    unsafe { crate::posix::ioctl_retry(fd, request.0, std::ptr::from_mut::<T>(arg).cast()) }
}

/// Issues a DRM ioctl whose structure carries user-space pointers.
///
/// # Safety
///
/// Every pointer field in `arg` must be valid for the kernel to read and
/// write for the element count passed beside it, for the whole call.
pub unsafe fn ioctl_with_buffers<T: Copy>(
    fd: std::ffi::c_int,
    request: u32,
    arg: &mut T,
) -> std::io::Result<()> {
    // SAFETY: the caller's guarantee.
    unsafe { crate::posix::ioctl_retry(fd, request, std::ptr::from_mut::<T>(arg).cast()) }
}

/// A buffer's address as the u64 the DRM structures carry.
pub fn ptr<T>(buffer: &mut [T]) -> u64 {
    buffer.as_mut_ptr() as u64
}

/// Safe entry points for the requests that carry user-space buffers. Each
/// passes the kernel a pointer and an element count taken from the same
/// slice, so the kernel never writes past what was lent to it.
pub mod safe {
    use super::{
        DRM_IOCTL_MODE_ATOMIC, DRM_IOCTL_MODE_CREATEPROPBLOB, DRM_IOCTL_MODE_GETCONNECTOR,
        DRM_IOCTL_MODE_GETPLANE, DRM_IOCTL_MODE_GETPLANERESOURCES, DRM_IOCTL_MODE_GETPROPERTY,
        DRM_IOCTL_MODE_GETRESOURCES, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, DRM_IOCTL_MODE_SETCRTC,
        drm_mode_atomic, drm_mode_card_res, drm_mode_create_blob, drm_mode_crtc,
        drm_mode_get_connector, drm_mode_get_plane, drm_mode_get_plane_res, drm_mode_get_property,
        drm_mode_modeinfo, drm_mode_obj_get_properties, drm_mode_property_enum, ioctl_with_buffers,
        ptr,
    };
    use std::ffi::c_int;
    use std::io;

    fn count<T>(slice: &[T]) -> u32 {
        u32::try_from(slice.len()).unwrap_or(u32::MAX)
    }

    /// The connector, with up to `modes.len()` of its modes written into
    /// `modes`. The returned `count_modes` may exceed the slice.
    pub fn get_connector(
        fd: c_int,
        id: u32,
        modes: &mut [drm_mode_modeinfo],
    ) -> io::Result<drm_mode_get_connector> {
        let mut info = drm_mode_get_connector {
            connector_id: id,
            count_modes: count(modes),
            modes_ptr: ptr(modes),
            ..Default::default()
        };
        // SAFETY: modes_ptr/count_modes describe `modes`; the other pointer
        // fields are zero with zero counts.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut info)? };
        Ok(info)
    }

    /// The card's CRTC ids into `crtcs`; returns how many the card has.
    pub fn get_crtcs(fd: c_int, crtcs: &mut [u32]) -> io::Result<u32> {
        let mut res = drm_mode_card_res {
            crtc_id_ptr: ptr(crtcs),
            count_crtcs: count(crtcs),
            ..Default::default()
        };
        // SAFETY: crtc_id_ptr/count_crtcs describe `crtcs`; every other array
        // has a zero count.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res)? };
        Ok(res.count_crtcs)
    }

    /// The card's plane ids into `planes`; returns how many it has.
    pub fn get_planes(fd: c_int, planes: &mut [u32]) -> io::Result<u32> {
        let mut res = drm_mode_get_plane_res {
            plane_id_ptr: ptr(planes),
            count_planes: count(planes),
            pad: 0,
        };
        // SAFETY: plane_id_ptr/count_planes describe `planes`.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &mut res)? };
        Ok(res.count_planes)
    }

    /// A plane, with its format list written into `formats`.
    pub fn get_plane(fd: c_int, id: u32, formats: &mut [u32]) -> io::Result<drm_mode_get_plane> {
        let mut plane = drm_mode_get_plane {
            plane_id: id,
            count_format_types: count(formats),
            format_type_ptr: ptr(formats),
            ..Default::default()
        };
        // SAFETY: format_type_ptr/count_format_types describe `formats`.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_GETPLANE, &mut plane)? };
        Ok(plane)
    }

    /// An object's property ids and values; returns how many it has.
    pub fn get_object_properties(
        fd: c_int,
        object: u32,
        object_type: u32,
        ids: &mut [u32],
        values: &mut [u64],
    ) -> io::Result<u32> {
        let n = count(ids).min(count(values));
        let mut get = drm_mode_obj_get_properties {
            props_ptr: ptr(ids),
            prop_values_ptr: ptr(values),
            count_props: n,
            obj_id: object,
            obj_type: object_type,
            pad: 0,
        };
        // SAFETY: both arrays hold at least `n` elements.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &mut get)? };
        Ok(get.count_props)
    }

    /// A property's name and flags, without its values or enum entries.
    pub fn get_property(fd: c_int, id: u32) -> io::Result<drm_mode_get_property> {
        let mut prop = drm_mode_get_property { prop_id: id, ..Default::default() };
        // SAFETY: both array counts are zero, so no pointer is followed.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_GETPROPERTY, &mut prop)? };
        Ok(prop)
    }

    /// An enum property's entries into `entries`; returns the property with
    /// its entry count.
    pub fn get_property_enums(
        fd: c_int,
        id: u32,
        entries: &mut [drm_mode_property_enum],
    ) -> io::Result<drm_mode_get_property> {
        let mut prop = drm_mode_get_property {
            prop_id: id,
            enum_blob_ptr: ptr(entries),
            count_enum_blobs: count(entries),
            ..Default::default()
        };
        // SAFETY: enum_blob_ptr/count_enum_blobs describe `entries`; the
        // value array's count is zero.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_GETPROPERTY, &mut prop)? };
        Ok(prop)
    }

    /// A property blob holding `mode`; returns its id.
    pub fn create_mode_blob(fd: c_int, mode: &drm_mode_modeinfo) -> io::Result<u32> {
        let mut blob = drm_mode_create_blob {
            data: std::ptr::from_ref(mode) as u64,
            length: u32::try_from(size_of::<drm_mode_modeinfo>()).unwrap_or(0),
            blob_id: 0,
        };
        // SAFETY: data/length describe `mode`, which the kernel only reads.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_CREATEPROPBLOB, &mut blob)? };
        Ok(blob.blob_id)
    }

    /// An atomic commit: `objects[i]` takes the next `counts[i]` entries of
    /// `props` and `values`.
    pub fn atomic_commit(
        fd: c_int,
        flags: u32,
        objects: &[u32],
        counts: &[u32],
        props: &[u32],
        values: &[u64],
    ) -> io::Result<()> {
        let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
        if objects.len() != counts.len()
            || props.len() != values.len()
            || total != props.len() as u64
        {
            return Err(io::Error::from_raw_os_error(22));
        }
        let mut atomic = drm_mode_atomic {
            flags,
            count_objs: count(objects),
            objs_ptr: objects.as_ptr() as u64,
            count_props_ptr: counts.as_ptr() as u64,
            props_ptr: props.as_ptr() as u64,
            prop_values_ptr: values.as_ptr() as u64,
            reserved: 0,
            user_data: 0,
        };
        // SAFETY: the kernel reads count_objs objects and counts, and as many
        // props and values as the counts sum to, which was checked above.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_ATOMIC, &mut atomic) }
    }

    /// The legacy modeset: `crtc` scans out `fb` in `mode` on `connectors`.
    pub fn set_crtc(
        fd: c_int,
        crtc: u32,
        fb: u32,
        connectors: &[u32],
        mode: &drm_mode_modeinfo,
    ) -> io::Result<()> {
        let mut set = drm_mode_crtc {
            set_connectors_ptr: connectors.as_ptr() as u64,
            count_connectors: count(connectors),
            crtc_id: crtc,
            fb_id: fb,
            mode_valid: 1,
            mode: *mode,
            ..Default::default()
        };
        // SAFETY: set_connectors_ptr/count_connectors describe `connectors`,
        // which the kernel only reads.
        unsafe { ioctl_with_buffers(fd, DRM_IOCTL_MODE_SETCRTC, &mut set) }
    }
}
