// Publishes the pose through the Proximity Core companion feed, beside the
// PXC_POSE export the roblox-tf bridge reads.
//
// The bridge path needs Proximity Core to inject into this process to read the
// export. The companion feed needs nothing injected anywhere: Proximity Core
// opens the shared-memory block by pid and copies it. Both run at once so an
// older Proximity Core keeps working.

use super::sdk::{self, valid, Sdk};
use super::tracker::Pose;
use std::sync::Mutex;

// The block carries studs, exactly what roblox-tf publishes: two players on the
// two paths have to agree on coordinates. The scale only tells Proximity Core how
// long a stud is; it never converts a position.
const STUDS_PER_METER: f32 = 1.0 / 0.28;

// Voice comes from the head, which sits above the root part along the
// character's up.
const HEAD_ABOVE_ROOT_STUDS: f32 = 1.5;

const DISPLAY_NAME: &str = "Roblox Pose Reader";
const DISCLAIMER: &str = "Roblox Pose Reader reads your character's position out of Roblox from \
    the outside and hands it to Proximity Core. Nothing is put into Roblox.";

static PUBLISHER: Mutex<Option<Sdk>> = Mutex::new(None);

/// The game Proximity Core offers to start the reader for. The Microsoft
/// Store client, `Windows10Universal.exe`, is left out: Proximity Core treats
/// that name as a generic Windows host and refuses it as a declared game.
const DECLARED_GAMES: [&str; 1] = ["RobloxPlayerBeta.exe"];

/// The picture on the reader's card in Proximity Core: the Roblox icon on
/// blue, centered because the card crops it to a wide banner.
const CARD_IMAGE: &[u8] = include_bytes!("../assets/card.png");

/// Start publishing. A failure is not fatal: the bridge path still works.
///
/// Opening also declares this executable as the program that starts the
/// reader. With the Roblox client declared as its game, Proximity Core can
/// start it, with no arguments, while Roblox runs: a plain start opens the
/// window and waits for Roblox, which is exactly what that needs.
pub fn open() {
    let publisher = match Sdk::load() {
        Ok(publisher) => publisher,
        Err(error) => return eprintln!("companion feed unavailable: {error}"),
    };
    if let Err(error) = publisher.open(DISPLAY_NAME, DISCLAIMER) {
        return eprintln!("companion feed unavailable: {error}");
    }
    if let Err(error) = publisher.declare_games(&DECLARED_GAMES) {
        eprintln!("companion games not declared: {error}");
    }
    if let Err(error) = publisher.set_image(CARD_IMAGE) {
        eprintln!("companion card image not published: {error}");
    }
    *lock() = Some(publisher);
}

/// Publish one frame. `None` for either pose withdraws it.
pub fn publish(roblox_pid: u32, player: Option<Pose>, camera: Option<Pose>) {
    let slot = lock();
    let Some(publisher) = slot.as_ref() else {
        return;
    };

    let mut pose = sdk::Pose::new();
    pose.valid = valid::UNITS_PER_METER | valid::LEVEL;
    pose.units_per_meter = STUDS_PER_METER;
    pose.set_level_name("Roblox");

    // The overlay has to cover Roblox; this process draws nothing of the game.
    if roblox_pid != 0 {
        pose.valid |= valid::OVERLAY_PID;
        pose.overlay_pid = roblox_pid;
    }

    if let Some(camera) = camera {
        pose.valid |= valid::CAMERA_POSITION | valid::CAMERA_BASIS;
        pose.camera_position = camera.pos;
        pose.camera_forward = camera.look;
        pose.camera_up = camera.up;
    }

    if let Some(player) = player {
        pose.valid |= valid::SPEAKER_POSITION | valid::SPEAKER_ORIENTATION;
        pose.speaker_position = head_of(&player);
        let pitch = player.look[1].clamp(-1.0, 1.0).asin();
        let yaw = (-player.look[0]).atan2(-player.look[2]);
        pose.speaker_orientation = [pitch, yaw, 0.0];
    }

    // Once open, publishing fails only on a malformed pose, which this never builds.
    let _ = publisher.publish(&pose);
}

fn head_of(player: &Pose) -> [f32; 3] {
    [
        player.pos[0] + player.up[0] * HEAD_ABOVE_ROOT_STUDS,
        player.pos[1] + player.up[1] * HEAD_ABOVE_ROOT_STUDS,
        player.pos[2] + player.up[2] * HEAD_ABOVE_ROOT_STUDS,
    ]
}

fn lock() -> std::sync::MutexGuard<'static, Option<Sdk>> {
    PUBLISHER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
