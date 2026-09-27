// The Proximity Core companion SDK: proximity_companion.dll, embedded in this
// exe by build.rs. pxc_companion.h in the SDK zip is the contract this file
// mirrors.
//
// A DLL has to be a file to load, so the embedded one is written out under
// %LOCALAPPDATA% first, unchanged, which keeps its signature intact. A failure
// on the way costs the companion feed and nothing else: the bridge path still
// works.

use std::ffi::{c_char, c_void, CStr, CString};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

const DLL_NAME: &str = "proximity_companion.dll";
const DLL_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/proximity_companion.dll"));
const DLL_SHA256: &str = env!("PXC_COMPANION_DLL_SHA256");

const LAYOUT_VERSION: u32 = 1;
const POSE_SIZE: usize = 872;
const LEVEL_NAME_CAPACITY: usize = 128;
const SURROUNDING_NAME_CAPACITY: usize = 64;
const SURROUNDING_SLOTS: usize = 8;

/// One bit per field a pose fills in. A cleared bit withdraws that field.
pub mod valid {
    pub const CAMERA_POSITION: u32 = 1 << 0;
    pub const CAMERA_BASIS: u32 = 1 << 1;
    pub const SPEAKER_POSITION: u32 = 1 << 4;
    pub const SPEAKER_ORIENTATION: u32 = 1 << 5;
    pub const LEVEL: u32 = 1 << 6;
    pub const UNITS_PER_METER: u32 = 1 << 7;
    pub const OVERLAY_PID: u32 = 1 << 9;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Surrounding {
    name: [c_char; SURROUNDING_NAME_CAPACITY],
    value: f32,
}

/// `PxcCompanionPose`.
#[repr(C)]
pub struct Pose {
    struct_size: u32,
    pub valid: u32,
    pub overlay_pid: u32,
    surrounding_count: u32,
    pub units_per_meter: f32,
    pub camera_fov_degrees: f32,
    pub camera_position: [f32; 3],
    pub camera_forward: [f32; 3],
    pub camera_up: [f32; 3],
    pub listener_position: [f32; 3],
    pub listener_orientation: [f32; 3],
    pub speaker_position: [f32; 3],
    pub speaker_orientation: [f32; 3],
    level_name: [c_char; LEVEL_NAME_CAPACITY],
    surroundings: [Surrounding; SURROUNDING_SLOTS],
    reserved: [u8; 92],
}

const _: () = assert!(std::mem::size_of::<Pose>() == POSE_SIZE);

impl Pose {
    pub fn new() -> Self {
        // SAFETY: every field is an integer, a float, or an array of them, so
        // all zeroes is a valid value.
        let mut pose: Self = unsafe { std::mem::zeroed() };
        pose.struct_size = POSE_SIZE as u32;
        pose
    }

    /// Truncates on a character boundary to fit, keeping the terminating NUL.
    pub fn set_level_name(&mut self, name: &str) {
        let mut end = name.len().min(LEVEL_NAME_CAPACITY - 1);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        self.level_name = [0; LEVEL_NAME_CAPACITY];
        for (slot, byte) in self.level_name.iter_mut().zip(&name.as_bytes()[..end]) {
            *slot = *byte as c_char;
        }
    }
}

extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// The loaded DLL's calls. The DLL is never unloaded.
pub struct Sdk {
    open: unsafe extern "C" fn(*const c_char, *const c_char) -> i32,
    publish: unsafe extern "C" fn(*const Pose) -> i32,
    declare_games: unsafe extern "C" fn(*const *const c_char, u32) -> i32,
    set_image: unsafe extern "C" fn(*const u8, u32) -> i32,
}

impl Sdk {
    /// Loads the embedded DLL by full path, never from the search path, and
    /// refuses one whose layout differs from the one this file mirrors.
    pub fn load() -> Result<Self, String> {
        let path = extract()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if module.is_null() {
            return Err(format!("{} did not load", path.display()));
        }
        unsafe {
            let layout_version: unsafe extern "C" fn() -> u32 =
                symbol(module, c"pxc_companion_layout_version")?;
            let pose_size: unsafe extern "C" fn() -> u32 =
                symbol(module, c"pxc_companion_pose_size")?;
            if layout_version() != LAYOUT_VERSION || pose_size() as usize != POSE_SIZE {
                return Err(format!("{DLL_NAME} publishes a layout this build does not know"));
            }
            Ok(Self {
                open: symbol(module, c"pxc_companion_open")?,
                publish: symbol(module, c"pxc_companion_publish")?,
                declare_games: symbol(module, c"pxc_companion_declare_games")?,
                set_image: symbol(module, c"pxc_companion_set_image")?,
            })
        }
    }

    pub fn open(&self, display_name: &str, disclaimer: &str) -> Result<(), String> {
        let display_name = c_string(display_name)?;
        let disclaimer = c_string(disclaimer)?;
        check(unsafe { (self.open)(display_name.as_ptr(), disclaimer.as_ptr()) })
    }

    pub fn publish(&self, pose: &Pose) -> Result<(), String> {
        check(unsafe { (self.publish)(pose) })
    }

    pub fn declare_games(&self, games: &[&str]) -> Result<(), String> {
        let names = games
            .iter()
            .map(|game| c_string(game))
            .collect::<Result<Vec<_>, _>>()?;
        let pointers: Vec<*const c_char> = names.iter().map(|name| name.as_ptr()).collect();
        check(unsafe { (self.declare_games)(pointers.as_ptr(), pointers.len() as u32) })
    }

    pub fn set_image(&self, bytes: &[u8]) -> Result<(), String> {
        let length = u32::try_from(bytes.len()).map_err(|_| "image too large".to_string())?;
        check(unsafe { (self.set_image)(bytes.as_ptr(), length) })
    }
}

/// Writes the embedded DLL to a folder named for its hash, so two builds with
/// different DLLs never share a file. A file already there with the right
/// bytes is reused: another running copy of this exe may have it loaded.
fn extract() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    let folder = base.join("RobloxPoseReader").join(&DLL_SHA256[..16]);
    let path = folder.join(DLL_NAME);
    if std::fs::read(&path).is_ok_and(|on_disk| on_disk == DLL_BYTES) {
        return Ok(path);
    }
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    std::fs::create_dir_all(&folder).map_err(failed)?;
    let staging = folder.join(format!("{DLL_NAME}.{}.tmp", std::process::id()));
    let written = std::fs::write(&staging, DLL_BYTES).and_then(|()| std::fs::rename(&staging, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written.map_err(failed)?;
    Ok(path)
}

/// # Safety
/// `F` must be the function pointer type the DLL exports under `name`.
unsafe fn symbol<F: Copy>(module: *mut c_void, name: &CStr) -> Result<F, String> {
    const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<*mut c_void>()) };
    let address = GetProcAddress(module, name.as_ptr());
    if address.is_null() {
        return Err(format!("{DLL_NAME} has no {}", name.to_string_lossy()));
    }
    Ok(std::mem::transmute_copy(&address))
}

fn c_string(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| format!("{text:?} contains a NUL"))
}

fn check(code: i32) -> Result<(), String> {
    let reason = match code {
        0 => return Ok(()),
        -1 => "bad argument",
        -2 => "already open",
        -3 => "operating system failure",
        -4 => "unsupported",
        -5 => "not open",
        _ => return Err(format!("unknown result {code}")),
    };
    Err(reason.to_string())
}
