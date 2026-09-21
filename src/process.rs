// Finding Roblox, opening it for reading, and walking its committed heap.

use std::ffi::c_void;

type Handle = *mut c_void;

#[allow(non_snake_case)]
#[repr(C)]
struct MemoryBasicInformation {
    base_address: usize,
    allocation_base: usize,
    allocation_protect: u32,
    _partition_id_and_pad: u32,
    region_size: usize,
    state: u32,
    protect: u32,
    mem_type: u32,
    _pad1: u32,
}

#[allow(non_snake_case)]
#[repr(C)]
struct ProcessEntry32W {
    dwSize: u32,
    cntUsage: u32,
    th32ProcessID: u32,
    th32DefaultHeapID: usize,
    th32ModuleID: u32,
    cntThreads: u32,
    th32ParentProcessID: u32,
    pcPriClassBase: i32,
    dwFlags: u32,
    szExeFile: [u16; 260],
}

extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn ReadProcessMemory(
        h: Handle,
        base: *const c_void,
        buf: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn VirtualQueryEx(
        h: Handle,
        addr: *const c_void,
        buf: *mut MemoryBasicInformation,
        len: usize,
    ) -> usize;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snap: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snap: Handle, entry: *mut ProcessEntry32W) -> i32;
}

const PROCESS_NAMES: [&str; 2] = ["RobloxPlayerBeta.exe", "Windows10Universal.exe"];

const PROCESS_VM_READ: u32 = 0x0010;
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const TH32CS_SNAPPROCESS: u32 = 0x0002;
const MEM_COMMIT: u32 = 0x1000;
const MEM_PRIVATE: u32 = 0x20000;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;

pub struct Target {
    handle: Handle,
    pub pid: u32,
}

impl Target {
    pub fn open(pid: u32) -> Option<Target> {
        let handle = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid) };
        if handle.is_null() {
            None
        } else {
            Some(Target { handle, pid })
        }
    }

    pub fn read_into(&self, addr: usize, buf: &mut [u8]) -> usize {
        let mut read: usize = 0;
        unsafe {
            ReadProcessMemory(
                self.handle,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut read as *mut usize,
            );
        }
        read
    }

    pub fn read_exact(&self, addr: usize, buf: &mut [u8]) -> bool {
        self.read_into(addr, buf) == buf.len()
    }

    pub fn is_running(&self) -> bool {
        const STILL_ACTIVE: u32 = 259;
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.handle, &mut code) != 0 && code == STILL_ACTIVE }
    }

    pub fn regions(&self) -> Vec<(usize, usize, u32)> {
        let mut out = Vec::new();
        let mut addr: usize = 0;
        let max: usize = 0x7fff_ffff_0000;
        loop {
            let mut mbi: MemoryBasicInformation = unsafe { std::mem::zeroed() };
            let n = unsafe {
                VirtualQueryEx(
                    self.handle,
                    addr as *const c_void,
                    &mut mbi as *mut MemoryBasicInformation,
                    std::mem::size_of::<MemoryBasicInformation>(),
                )
            };
            if n == 0 {
                break;
            }
            if mbi.region_size == 0 {
                break;
            }
            let heap = mbi.state == MEM_COMMIT
                && mbi.mem_type == MEM_PRIVATE
                && (mbi.protect == PAGE_READWRITE || mbi.protect == PAGE_EXECUTE_READWRITE);
            if heap {
                out.push((mbi.base_address, mbi.region_size, mbi.protect));
            }
            let next = mbi.base_address.saturating_add(mbi.region_size);
            if next <= addr || next >= max {
                break;
            }
            addr = next;
        }
        out
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub fn find_roblox_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() {
        return pids;
    }
    let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<ProcessEntry32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
    while ok != 0 {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if PROCESS_NAMES.iter().any(|known| name.eq_ignore_ascii_case(known)) {
            pids.push(entry.th32ProcessID);
        }
        ok = unsafe { Process32NextW(snap, &mut entry) };
    }
    unsafe {
        CloseHandle(snap);
    }
    pids
}

pub fn orthonormal(m: &[f32; 9]) -> bool {
    for v in m {
        if !v.is_finite() {
            return false;
        }
    }
    let r = [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let tol = 0.01f32;
    let n0 = dot(r[0], r[0]);
    let n1 = dot(r[1], r[1]);
    let n2 = dot(r[2], r[2]);
    if (n0 - 1.0).abs() > tol || (n1 - 1.0).abs() > tol || (n2 - 1.0).abs() > tol {
        return false;
    }
    if dot(r[0], r[1]).abs() > tol || dot(r[0], r[2]).abs() > tol || dot(r[1], r[2]).abs() > tol {
        return false;
    }
    // Proper rotation only (determinant +1), rejects reflections / coincidences.
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
        - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    (det - 1.0).abs() < 0.05
}

