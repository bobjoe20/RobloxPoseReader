# Roblox Pose Reader

Shows where your Roblox character and camera are, and publishes both so another
program can use them. It exists to give Proximity Core spatial audio in Roblox.

Nothing is injected into Roblox and nothing is written to it. The tool opens a
read handle on the process and reads memory, the way a debugger would.

Reading Roblox's memory is against Roblox's terms even though it only reads.
Your account could be banned. Use one you can afford to lose.

## Download

Grab `pxc-roblox.exe` from the [latest release](../../releases/latest). It is a
single file, needs no install, and has no dependencies beyond Windows itself.

It is not code signed, so Windows shows "Windows protected your PC" on the first
run. Every release is built here by GitHub Actions and carries a provenance
attestation, so you can check the binary you downloaded was built from this
source and nothing else:

```
gh attestation verify pxc-roblox.exe --repo bobjoe20/RobloxPoseReader
```

## Use

Start it any time, before or after Roblox. It waits for the game, finds your
character, and tracks it. The top table is the live pose; the plan view below is
top-down with your heading and the camera's field of view. The mouse wheel zooms.

## How it finds you

Roblox stores every part's position and rotation as a CFrame, and every object as
an Instance with a name. The tool walks that tree from `Players` to the local
`Player`, to its `Character`, to `HumanoidRootPart`, and reads the CFrame from the
part's physics object. The camera comes from `Workspace.CurrentCamera`.

No offsets are hardcoded. An Instance gives itself away by holding a pointer to
itself at +8, and each field offset is worked out by finding many instances with a
known name and taking the offset most of them agree on. That means a Roblox update
which moves fields around costs nothing. A change to the shape of those structures
is what would break it, and the tool then says which step failed rather than
reporting a wrong position.

## Publishing the pose

The exe exports a 96-byte block named `PXC_POSE` holding both poses. A reader
resolves it from the export table. The sequence counter is odd while a write is in
progress and moves on every publish, so a consumer can reject a torn read and can
tell when this process has stalled. `src/export.rs` has the layout.

## Build

Needs the Rust toolchain and the MSVC build tools. No other dependencies.

```
cargo build --release
```

The binary lands in `target/x86_64-pc-windows-msvc/release/pxc-roblox.exe`.
Windows on ARM builds it too; the x64 binary reads the x64 game.

## Diagnostics

```
pxc-roblox.exe graph [seconds]      resolve, print how each offset was derived
pxc-roblox.exe camera               list every CFrame inside the camera object
pxc-roblox.exe dump 0x<addr> [span] annotated pointer dump
```
