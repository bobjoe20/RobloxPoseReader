// The exported pose block another process reads by symbol name.
//
// 96 bytes, little-endian: magic "PXPF", layout version, sequence (odd while a
// write is in progress, bumped on every publish), flags (bit 0 player, bit 1
// camera), Roblox pid, reserved, then 18 floats: player and camera position,
// look and up. Studs, Y up, -Z forward.

use super::tracker::Pose;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};

const MAGIC: u32 = u32::from_le_bytes(*b"PXPF");
const LAYOUT_VERSION: u32 = 1;
const FLAG_PLAYER: u32 = 1;
const FLAG_CAMERA: u32 = 2;

#[repr(C)]
pub struct PoseBlock {
    magic: u32,
    version: u32,
    sequence: AtomicU32,
    body: UnsafeCell<Body>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Body {
    flags: u32,
    roblox_pid: u32,
    reserved: u32,
    floats: [f32; 18],
}

// Only the tracker thread writes, and only between the two sequence bumps.
unsafe impl Sync for PoseBlock {}

#[no_mangle]
#[used]
pub static PXC_POSE: PoseBlock = PoseBlock {
    magic: MAGIC,
    version: LAYOUT_VERSION,
    sequence: AtomicU32::new(0),
    body: UnsafeCell::new(Body { flags: 0, roblox_pid: 0, reserved: 0, floats: [0.0; 18] }),
};

pub fn publish(roblox_pid: u32, player: Option<Pose>, camera: Option<Pose>) {
    let mut body = Body { flags: 0, roblox_pid, reserved: 0, floats: [0.0; 18] };
    for (slot, flag, pose) in [(0, FLAG_PLAYER, player), (9, FLAG_CAMERA, camera)] {
        let Some(pose) = pose else { continue };
        body.flags |= flag;
        body.floats[slot..slot + 3].copy_from_slice(&pose.pos);
        body.floats[slot + 3..slot + 6].copy_from_slice(&pose.look);
        body.floats[slot + 6..slot + 9].copy_from_slice(&pose.up);
    }
    PXC_POSE.sequence.fetch_add(1, Ordering::SeqCst);
    unsafe { std::ptr::write_volatile(PXC_POSE.body.get(), body) };
    PXC_POSE.sequence.fetch_add(1, Ordering::SeqCst);
}
