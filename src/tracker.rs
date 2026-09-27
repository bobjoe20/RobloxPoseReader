// Background thread: wait for Roblox, resolve, publish the pose at 60 Hz.

use super::companion;
use super::export;
use super::graph::{self, Resolved, RootPose};
use super::process::{find_roblox_pids, Target};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy)]
pub struct Pose {
    pub pos: [f32; 3],
    pub look: [f32; 3],
    pub up: [f32; 3],
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub status: String,
    pub tracking: bool,
    pub pid: u32,
    pub user: String,
    pub player: Option<Pose>,
    pub camera: Option<Pose>,
}

pub type Shared = Arc<Mutex<Snapshot>>;

const TICK: Duration = Duration::from_millis(16);
const TICKS_PER_REFRESH: u32 = 30;
const LOST_POSE_TICKS: u32 = 300;

// Roblox LookVector is minus the third column of the row-major rotation.
pub fn look_vector(rot: &[f32; 9]) -> [f32; 3] {
    [-rot[2], -rot[5], -rot[8]]
}

// UpVector is the second column.
fn up_vector(rot: &[f32; 9]) -> [f32; 3] {
    [rot[1], rot[4], rot[7]]
}

fn to_pose(root: RootPose) -> Pose {
    Pose { pos: root.pos, look: look_vector(&root.rot), up: up_vector(&root.rot) }
}

fn publish(shared: &Shared, update: impl FnOnce(&mut Snapshot)) {
    if let Ok(mut snapshot) = shared.lock() {
        update(&mut snapshot);
    }
}

fn set_status(shared: &Shared, status: String) {
    publish(shared, |s| {
        s.status = status;
        s.tracking = false;
        s.player = None;
        s.camera = None;
    });
    export::publish(0, None, None);
    companion::publish(0, None, None);
}

pub fn spawn(shared: Shared) {
    companion::open();
    thread::spawn(move || loop {
        let pids = find_roblox_pids();
        if pids.is_empty() {
            set_status(&shared, "Waiting for Roblox. Join a game.".into());
            thread::sleep(Duration::from_secs(1));
            continue;
        }
        let mut last_reason = String::new();
        for pid in pids {
            let Some(target) = Target::open(pid) else {
                last_reason = format!("Cannot open Roblox (pid {pid}). Try running as administrator.");
                continue;
            };
            set_status(&shared, format!("Found Roblox (pid {pid}). Locating your character…"));
            match graph::resolve(&target) {
                Ok(resolved) => track(&shared, &target, resolved),
                Err(reason) => last_reason = reason,
            }
        }
        if !last_reason.is_empty() {
            set_status(&shared, last_reason);
        }
        thread::sleep(Duration::from_secs(2));
    });
}

fn track(shared: &Shared, target: &Target, mut resolved: Resolved) {
    let user = resolved.player_name(target);
    publish(shared, |s| {
        s.status = "Tracking".into();
        s.tracking = true;
        s.pid = target.pid;
        s.user = user;
    });
    let mut tick = 0u32;
    let mut ticks_without_pose = 0u32;
    loop {
        tick = tick.wrapping_add(1);
        if tick % TICKS_PER_REFRESH == 0 && (!target.is_running() || !resolved.refresh(target)) {
            return;
        }
        let player = resolved.read_pose(target).map(to_pose);
        let camera = resolved.read_camera_pose(target).map(to_pose);
        ticks_without_pose = if player.is_some() { 0 } else { ticks_without_pose + 1 };
        if ticks_without_pose > LOST_POSE_TICKS {
            return;
        }
        publish(shared, |s| {
            s.player = player;
            s.camera = camera;
        });
        export::publish(target.pid, player, camera);
        companion::publish(target.pid, player, camera);
        thread::sleep(TICK);
    }
}
