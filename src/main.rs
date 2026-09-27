// Reads the local player's pose out of a running Roblox process. Nothing is
// injected; the tool opens a read handle and reads memory.
//
// No arguments opens the window. Diagnostics: graph [seconds], camera,
// dump 0x<addr> [span].

#![windows_subsystem = "windows"]

mod companion;
mod export;
mod graph;
mod gui;
mod process;
mod sdk;
mod tracker;

use process::{find_roblox_pids, orthonormal, Target};
use std::time::{Duration, Instant};
use tracker::look_vector;

extern "system" {
    fn AttachConsole(pid: u32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None => gui::run(),
        Some(mode) => {
            const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
            unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
            diagnostics(mode, &args);
        }
    }
}

fn parse_hex(text: &str) -> usize {
    usize::from_str_radix(text.trim_start_matches("0x"), 16).expect("hex value, e.g. 0x1A2B3C")
}

fn diagnostics(mode: &str, args: &[String]) {
    let Some(target) = find_roblox_pids().into_iter().find_map(Target::open) else {
        eprintln!("No readable Roblox process found.");
        return;
    };
    eprintln!("Roblox pid={}", target.pid);
    if mode == "dump" {
        let span = args.get(3).map_or(0x200, |s| parse_hex(s));
        graph::dump(&target, parse_hex(&args[2]), span);
        return;
    }
    let mut resolved = match graph::resolve(&target) {
        Ok(resolved) => resolved,
        Err(reason) => return eprintln!("not resolved: {reason}"),
    };
    if mode == "camera" {
        if let Some((camera, found)) = graph::camera_cframes(&target, &resolved) {
            println!("camera 0x{camera:012X}");
            for (off, pose) in found {
                println!("  +0x{off:03X} pos={:?} look={:?}", pose.pos, look_vector(&pose.rot));
            }
        }
        return;
    }
    let seconds = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        resolved.refresh(&target);
        if let Some(pose) = resolved.read_pose(&target) {
            println!("player pos={:?} look={:?}", pose.pos, look_vector(&pose.rot));
        }
        if let Some(pose) = resolved.read_camera_pose(&target) {
            println!("camera pos={:?} look={:?}", pose.pos, look_vector(&pose.rot));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
