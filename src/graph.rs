// Resolves the local character's root part by walking Roblox's instance tree.
// Nothing is hardcoded: an Instance is recognised by the self pointer at +8, and
// every field offset is voted on across the instances found by name.

use super::{orthonormal, Target};
use std::collections::{HashMap, HashSet};

const ROOT_PART_NAME: &str = "HumanoidRootPart";
const PLAYERS_NAME: &str = "Players";
const SELF_POINTER_OFFSET: usize = 8;
const INSTANCE_HEADER_SPAN: usize = 0x200;
const MAX_PARENT_HOPS: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct GraphLayout {
    pub name_off: usize,
    pub name_string_off: usize,
    pub class_off: usize,
    pub parent_off: usize,
    pub children_off: Option<usize>,
    pub primitive_off: usize,
    pub cframe_off: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Resolved {
    pub layout: GraphLayout,
    pub player: usize,
    pub character_off: usize,
    pub character: usize,
    pub root_part: usize,
    pub camera: Option<CameraLock>,
}

#[derive(Clone, Copy, Debug)]
pub struct CameraLock {
    pub camera: usize,
    pub cframe_off: usize,
}

pub struct RootPose {
    pub pos: [f32; 3],
    pub rot: [f32; 9],
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn read_u64(t: &Target, addr: usize) -> Option<u64> {
    let mut b = [0u8; 8];
    t.read_exact(addr, &mut b).then(|| u64::from_le_bytes(b))
}

fn read_span(t: &Target, addr: usize, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let mut got = t.read_into(addr, &mut buf);
    if got == 0 {
        // Partial copies can report zero; retry up to the next page boundary.
        let to_page_end = 0x1000 - (addr & 0xfff);
        if to_page_end < len {
            got = t.read_into(addr, &mut buf[..to_page_end]);
        }
    }
    buf.truncate(got);
    buf
}

fn is_heap_pointer(v: u64) -> bool {
    (0x10000..0x7fff_0000_0000).contains(&v) && v % 8 == 0
}

fn is_instance(t: &Target, addr: usize) -> bool {
    is_heap_pointer(addr as u64)
        && read_u64(t, addr + SELF_POINTER_OFFSET) == Some(addr as u64)
}

// MSVC std::string: 16-byte inline buffer or heap pointer, then size, then capacity.
fn read_std_string(t: &Target, addr: usize) -> Option<String> {
    let mut b = [0u8; 32];
    if !t.read_exact(addr, &mut b) {
        return None;
    }
    let size = u64_at(&b, 16) as usize;
    let capacity = u64_at(&b, 24) as usize;
    if size > 256 || capacity < size {
        return None;
    }
    if capacity < 16 {
        return String::from_utf8(b[..size].to_vec()).ok();
    }
    let data = read_span(t, u64_at(&b, 0) as usize, size);
    (data.len() == size).then(|| String::from_utf8(data).ok()).flatten()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NameField {
    pub off: usize,
    pub string_off: usize,
}

pub fn instance_name(t: &Target, field: NameField, inst: usize) -> Option<String> {
    let record = read_u64(t, inst + field.off)?;
    if !is_heap_pointer(record) {
        return None;
    }
    read_std_string(t, record as usize + field.string_off)
}

const CLASS_NAME_POINTER_OFFSET: usize = 8;

fn class_name(t: &Target, class_off: usize, inst: usize) -> Option<String> {
    let descriptor = read_u64(t, inst + class_off)? as usize;
    let string_object = read_u64(t, descriptor + CLASS_NAME_POINTER_OFFSET)?;
    read_std_string(t, string_object as usize)
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn derive_class_offset(t: &Target, instances: &[usize]) -> Option<usize> {
    let votes = instances.iter().flat_map(|&inst| {
        (0x10..0x60usize).step_by(8).filter(move |&off| class_name(t, off, inst).is_some())
    });
    mode(votes).map(|(off, n)| {
        eprintln!("  class offset 0x{off:X} ({n}/{} instances)", instances.len());
        off
    })
}

impl GraphLayout {
    fn name_field(&self) -> NameField {
        NameField { off: self.name_off, string_off: self.name_string_off }
    }
}

fn for_each_chunk(t: &Target, mut visit: impl FnMut(&[u8], usize)) {
    const CHUNK: usize = 4 * 1024 * 1024;
    const OVERLAP: usize = 32;
    const FALLBACK: usize = 64 * 1024;
    let mut buf = vec![0u8; CHUNK];
    for (base, size, _) in t.regions() {
        let mut off = 0usize;
        while off < size {
            let want = (size - off).min(CHUNK);
            let got = t.read_into(base + off, &mut buf[..want]);
            if got >= OVERLAP {
                visit(&buf[..got], base + off);
            }
            let mut sub = got & !(FALLBACK - 1);
            while got < want && sub < want {
                let w = (want - sub).min(FALLBACK);
                let m = t.read_into(base + off + sub, &mut buf[sub..sub + w]);
                if m >= OVERLAP {
                    visit(&buf[sub..sub + m], base + off + sub);
                }
                sub += w;
            }
            if want < CHUNK {
                break;
            }
            off += CHUNK - OVERLAP;
        }
    }
}

struct NameAnchors {
    root_part_text: Vec<u64>,    // heap buffers holding "HumanoidRootPart\0"
    players_strings: Vec<u64>,   // std::string objects holding "Players" inline
}

fn find_name_anchors(t: &Target) -> NameAnchors {
    let mut text = ROOT_PART_NAME.as_bytes().to_vec();
    text.push(0);
    let text_head = u64_at(&text, 0);
    let mut players_inline = [0u8; 8];
    players_inline[..7].copy_from_slice(PLAYERS_NAME.as_bytes());
    let players_head = u64::from_le_bytes(players_inline);

    let mut anchors = NameAnchors { root_part_text: Vec::new(), players_strings: Vec::new() };
    for_each_chunk(t, |buf, base| {
        let mut i = 0usize;
        while i + 32 <= buf.len() {
            let q = u64_at(buf, i);
            if q == text_head && buf[i..i + text.len()] == text[..] {
                anchors.root_part_text.push((base + i) as u64);
            } else if q == players_head && u64_at(buf, i + 16) == 7 && u64_at(buf, i + 24) == 15 {
                anchors.players_strings.push((base + i) as u64);
            }
            i += 8;
        }
    });
    anchors.root_part_text.sort_unstable();
    anchors.root_part_text.dedup();
    anchors.players_strings.sort_unstable();
    anchors.players_strings.dedup();
    anchors
}

fn find_pointers_to(t: &Target, targets: &HashSet<u64>) -> Vec<(usize, u64)> {
    let mut hits = Vec::new();
    let (Some(&lo), Some(&hi)) = (targets.iter().min(), targets.iter().max()) else {
        return hits;
    };
    for_each_chunk(t, |buf, base| {
        let mut i = 0usize;
        while i + 8 <= buf.len() {
            let q = u64_at(buf, i);
            if q >= lo && q <= hi && targets.contains(&q) {
                hits.push((base + i, q));
            }
            i += 8;
        }
    });
    hits.sort_unstable();
    hits.dedup();
    hits
}

fn name_field_distance(t: &Target, slot: usize) -> Option<usize> {
    let start = slot.checked_sub(INSTANCE_HEADER_SPAN)?;
    let buf = read_span(t, start, INSTANCE_HEADER_SPAN + 8);
    if buf.len() < INSTANCE_HEADER_SPAN + 8 {
        return None;
    }
    (16..=INSTANCE_HEADER_SPAN).step_by(8).find(|&k| {
        let base = slot - k;
        u64_at(&buf, INSTANCE_HEADER_SPAN - k + SELF_POINTER_OFFSET) == base as u64
    })
}

fn mode<T: std::hash::Hash + Eq + Copy + Ord>(values: impl Iterator<Item = T>) -> Option<(T, usize)> {
    let mut counts: HashMap<T, usize> = HashMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    counts.into_iter().max_by_key(|&(v, n)| (n, std::cmp::Reverse(v)))
}

fn parent_chain_depth(t: &Target, inst: usize, parent_off: usize) -> Option<usize> {
    let mut cur = inst;
    for depth in 0..MAX_PARENT_HOPS {
        let next = read_u64(t, cur + parent_off)? as usize;
        if next == 0 {
            return (depth > 0).then_some(depth);
        }
        if next == cur || !is_instance(t, next) {
            return None;
        }
        cur = next;
    }
    None
}

fn derive_parent_offset(t: &Target, instances: &[usize], name_off: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for off in (0x10..0x100).step_by(8).filter(|&o| o != name_off) {
        let score = instances
            .iter()
            .filter(|&&inst| parent_chain_depth(t, inst, off).is_some_and(|d| d >= 2))
            .count();
        if score > best.map_or(0, |(_, s)| s) {
            best = Some((off, score));
        }
    }
    best.map(|(off, score)| {
        eprintln!("  parent offset 0x{off:X} ({score}/{} root parts chain to a root)", instances.len());
        off
    })
}

pub fn path_of(t: &Target, layout: &GraphLayout, inst: usize) -> String {
    let mut names = Vec::new();
    let mut cur = inst;
    for _ in 0..MAX_PARENT_HOPS {
        if cur == 0 {
            break;
        }
        names.push(instance_name(t, layout.name_field(), cur).unwrap_or_else(|| "?".into()));
        cur = read_u64(t, cur + layout.parent_off).unwrap_or(0) as usize;
    }
    names.reverse();
    names.join("/")
}

fn children_of(t: &Target, children_off: usize, parent: usize) -> Vec<usize> {
    let Some(vector) = read_u64(t, parent + children_off).filter(|&v| is_heap_pointer(v)) else {
        return Vec::new();
    };
    let (Some(begin), Some(end)) = (read_u64(t, vector as usize), read_u64(t, vector as usize + 8)) else {
        return Vec::new();
    };
    let bytes = end.wrapping_sub(begin) as usize;
    if !is_heap_pointer(begin) || bytes == 0 || bytes % 16 != 0 || bytes > 16 * 8192 {
        return Vec::new();
    }
    let buf = read_span(t, begin as usize, bytes);
    buf.chunks_exact(16).map(|entry| u64_at(entry, 0) as usize).collect()
}

fn derive_children_offset(t: &Target, child_parent_pairs: &[(usize, usize)]) -> Option<usize> {
    let votes = child_parent_pairs.iter().flat_map(|&(child, parent)| {
        (0x10..0x100usize)
            .step_by(8)
            .filter(move |&off| children_of(t, off, parent).contains(&child))
    });
    mode(votes).map(|(off, n)| {
        eprintln!("  children offset 0x{off:X} ({n}/{} parents list their root part)", child_parent_pairs.len());
        off
    })
}

fn read_rot_first_cframe(t: &Target, addr: usize) -> Option<RootPose> {
    let mut b = [0u8; 48];
    if !t.read_exact(addr, &mut b) {
        return None;
    }
    parse_rot_first(&b)
}

fn parse_rot_first(b: &[u8]) -> Option<RootPose> {
    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let mut rot = [0f32; 9];
    for (i, r) in rot.iter_mut().enumerate() {
        *r = f(i * 4);
    }
    let pos = [f(36), f(40), f(44)];
    let sane = pos.iter().all(|v| v.is_finite() && v.abs() < 1.0e6);
    (sane && orthonormal(&rot)).then_some(RootPose { pos, rot })
}

fn derive_primitive_offsets(t: &Target, root_parts: &[usize]) -> Option<(usize, usize)> {
    const PART_SPAN: usize = 0x400;
    const PRIMITIVE_SPAN: usize = 0x300;
    let mut votes: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for &part in root_parts {
        let body = read_span(t, part, PART_SPAN);
        for off in (0x40..body.len().saturating_sub(7)).step_by(8) {
            let target = u64_at(&body, off);
            // The primitive is a plain physics object; Instances (the parent Model
            // carries a pivot CFrame and a PrimaryPart pointer) are not it.
            if !is_heap_pointer(target) || target as usize == part || is_instance(t, target as usize) {
                continue;
            }
            let primitive = read_span(t, target as usize, PRIMITIVE_SPAN);
            let points_back = primitive
                .chunks_exact(8)
                .any(|q| u64::from_le_bytes(q.try_into().unwrap()) == part as u64);
            for cframe_off in (0..primitive.len().saturating_sub(47)).step_by(4) {
                let Some(pose) = parse_rot_first(&primitive[cframe_off..cframe_off + 48]) else {
                    continue;
                };
                if pose.pos == [0.0; 3] {
                    continue;
                }
                let entry = votes.entry((off, cframe_off)).or_default();
                entry.0 += 1;
                entry.1 += points_back as usize;
            }
        }
    }
    let mut ranked: Vec<_> = votes.into_iter().collect();
    ranked.sort_by_key(|&((off, cf), (n, back))| (std::cmp::Reverse(back), std::cmp::Reverse(n), off, cf));
    for ((off, cf), (n, back)) in ranked.iter().take(6) {
        eprintln!("  primitive candidate part+0x{off:X} -> +0x{cf:X}: {n} parts, {back} point back");
    }
    ranked.first().map(|&(key, _)| key)
}

fn instance_fields(t: &Target, owner: usize, span: usize, accept: impl Fn(usize) -> bool) -> Vec<(usize, usize)> {
    let body = read_span(t, owner, span);
    (0x10..body.len().saturating_sub(7))
        .step_by(8)
        .map(|off| (off, u64_at(&body, off) as usize))
        .filter(|&(_, target)| target != owner && accept(target))
        .collect()
}

const NAME_STRING_OFFSETS: [u64; 3] = [0, 8, 16];

fn with_record_variants(strings: impl Iterator<Item = u64>) -> HashSet<u64> {
    strings.flat_map(|s| NAME_STRING_OFFSETS.map(|delta| s - delta)).collect()
}

fn instances_named(t: &Target, slots: &[(usize, u64)], strings: &HashSet<u64>, field: NameField) -> Vec<usize> {
    let mut out: Vec<usize> = slots
        .iter()
        .filter(|&&(_, record)| strings.contains(&(record + field.string_off as u64)))
        .filter(|&&(slot, _)| name_field_distance(t, slot) == Some(field.off))
        .map(|&(slot, _)| slot - field.off)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub fn resolve(t: &Target) -> Result<Resolved, String> {
    eprintln!("[1/3] sweeping heap for name anchors…");
    let anchors = find_name_anchors(t);
    eprintln!(
        "  {} \"{ROOT_PART_NAME}\" buffers, {} \"{PLAYERS_NAME}\" strings",
        anchors.root_part_text.len(),
        anchors.players_strings.len()
    );
    if anchors.root_part_text.is_empty() {
        return Err("no HumanoidRootPart name in memory — is a character spawned?".into());
    }

    eprintln!("[2/3] sweeping for root-part string objects…");
    let text_set: HashSet<u64> = anchors.root_part_text.iter().copied().collect();
    let root_part_strings: HashSet<u64> = find_pointers_to(t, &text_set)
        .into_iter()
        .filter(|&(slot, _)| read_u64(t, slot + 16) == Some(ROOT_PART_NAME.len() as u64))
        .map(|(slot, _)| slot as u64)
        .collect();
    let players_strings: HashSet<u64> = anchors.players_strings.iter().copied().collect();
    eprintln!("  {} root-part string objects", root_part_strings.len());

    eprintln!("[3/3] sweeping for name fields…");
    let all_strings: HashSet<u64> = root_part_strings.union(&players_strings).copied().collect();
    let slots = find_pointers_to(t, &with_record_variants(all_strings.iter().copied()));
    eprintln!("  {} slots point at a name record", slots.len());

    let field_votes = slots.iter().flat_map(|&(slot, record)| {
        let distance = name_field_distance(t, slot);
        let all_strings = &all_strings;
        NAME_STRING_OFFSETS.into_iter().filter_map(move |delta| {
            let off = distance?;
            all_strings
                .contains(&(record + delta))
                .then_some(NameField { off, string_off: delta as usize })
        })
    });
    let (field, votes) = mode(field_votes).ok_or("no name slot sits inside an Instance (self pointer not found)")?;
    eprintln!("  name field {field:X?} ({votes} votes)");
    let name_off = field.off;

    let named_root_part = instances_named(t, &slots, &root_part_strings, field);
    let players_services = instances_named(t, &slots, &players_strings, field);
    let class_off = derive_class_offset(t, &named_root_part).ok_or("no class descriptor offset found")?;
    let root_parts: Vec<usize> = named_root_part
        .iter()
        .copied()
        .filter(|&inst| class_name(t, class_off, inst).is_some_and(|class| class.ends_with("Part")))
        .collect();
    eprintln!(
        "  {} Instances named {ROOT_PART_NAME} ({} are parts), {} named {PLAYERS_NAME}",
        named_root_part.len(),
        root_parts.len(),
        players_services.len()
    );

    let parent_off = derive_parent_offset(t, &root_parts, name_off).ok_or("no parent offset found")?;
    let live_root_parts: Vec<(usize, usize)> = root_parts
        .iter()
        .filter(|&&part| parent_chain_depth(t, part, parent_off).is_some())
        .filter_map(|&part| read_u64(t, part + parent_off).map(|parent| (part, parent as usize)))
        .collect();
    let children_off = derive_children_offset(t, &live_root_parts);
    let parts_only: Vec<usize> = live_root_parts.iter().map(|&(part, _)| part).collect();
    let (primitive_off, cframe_off) =
        derive_primitive_offsets(t, &parts_only).ok_or("no primitive/CFrame path found from any root part")?;
    let layout = GraphLayout {
        name_off,
        name_string_off: field.string_off,
        class_off,
        parent_off,
        children_off,
        primitive_off,
        cframe_off,
    };
    eprintln!("  layout: {layout:X?}");
    for &(part, _) in &live_root_parts {
        eprintln!("    root part 0x{part:012X}  {}", path_of(t, &layout, part));
    }

    let character_of: HashMap<usize, usize> = live_root_parts.iter().map(|&(part, parent)| (parent, part)).collect();
    for &players in &players_services {
        if parent_chain_depth(t, players, parent_off) != Some(1) {
            continue;
        }
        let local_players = instance_fields(t, players, 0x600, |target| {
            is_instance(t, target) && read_u64(t, target + parent_off) == Some(players as u64)
        });
        for (local_off, player) in local_players {
            let name = instance_name(t, field, player).unwrap_or_default();
            eprintln!("  Players 0x{players:012X} +0x{local_off:X} -> LocalPlayer \"{name}\" 0x{player:012X}");
            let characters = instance_fields(t, player, 0x1000, |target| character_of.contains_key(&target));
            if let Some(&(character_off, character)) = characters.first() {
                let root_part = character_of[&character];
                eprintln!("  Player +0x{character_off:X} -> Character -> {}", path_of(t, &layout, root_part));
                let mut resolved = Resolved { layout, player, character_off, character, root_part, camera: None };
                resolved.camera = locate_camera(t, &resolved);
                match resolved.camera {
                    Some(lock) => eprintln!("  CurrentCamera 0x{:012X}, CFrame at +0x{:X}", lock.camera, lock.cframe_off),
                    None => eprintln!("  CurrentCamera not found; camera pose unavailable"),
                }
                return Ok(resolved);
            }
        }
    }
    Err("found root parts but no LocalPlayer whose Character owns one — character not spawned yet?".into())
}

// Camera.CFrame sits directly before Camera.Focus and looks at it, or sits on
// it in first person.
fn locate_camera(t: &Target, resolved: &Resolved) -> Option<CameraLock> {
    const CFRAME_BYTES: usize = 48;
    const FIRST_PERSON_RANGE: f32 = 1.5;
    let (camera, found) = camera_cframes(t, resolved)?;
    found.iter().find_map(|(off, pose)| {
        let (_, focus) = found.iter().find(|(focus_off, _)| *focus_off == off + CFRAME_BYTES)?;
        let to_focus = [focus.pos[0] - pose.pos[0], focus.pos[1] - pose.pos[1], focus.pos[2] - pose.pos[2]];
        let range = to_focus.iter().map(|v| v * v).sum::<f32>().sqrt();
        let look = [-pose.rot[2], -pose.rot[5], -pose.rot[8]];
        let aligned = (0..3).map(|i| to_focus[i] * look[i]).sum::<f32>() / range.max(1e-6);
        (range < FIRST_PERSON_RANGE || aligned > 0.98).then_some(CameraLock { camera, cframe_off: *off })
    })
}

impl Resolved {
    pub fn read_pose(&self, t: &Target) -> Option<RootPose> {
        let primitive = read_u64(t, self.root_part + self.layout.primitive_off)?;
        if !is_heap_pointer(primitive) {
            return None;
        }
        read_rot_first_cframe(t, primitive as usize + self.layout.cframe_off)
    }

    pub fn player_name(&self, t: &Target) -> String {
        instance_name(t, self.layout.name_field(), self.player).unwrap_or_default()
    }

    pub fn read_camera_pose(&self, t: &Target) -> Option<RootPose> {
        let lock = self.camera?;
        read_rot_first_cframe(t, lock.camera + lock.cframe_off)
    }

    // Follows Player.Character again so a respawn is picked up by pointer walk.
    pub fn refresh(&mut self, t: &Target) -> bool {
        if !is_instance(t, self.player) {
            return false;
        }
        if let Some(lock) = locate_camera(t, self) {
            self.camera = Some(lock);
        }
        let character = read_u64(t, self.player + self.character_off).unwrap_or(0) as usize;
        let parent_now = read_u64(t, self.root_part + self.layout.parent_off).unwrap_or(0) as usize;
        if character == self.character && parent_now == character {
            return true;
        }
        let Some(children_off) = self.layout.children_off else {
            return character == 0;
        };
        if character != 0 {
            let root_part = children_of(t, children_off, character)
                .into_iter()
                .find(|&child| instance_name(t, self.layout.name_field(), child).as_deref() == Some(ROOT_PART_NAME));
            if let Some(root_part) = root_part {
                eprintln!("character changed -> root part 0x{root_part:012X}");
                self.character = character;
                self.root_part = root_part;
            }
        }
        true
    }
}

const CAMERA_CLASS: &str = "Camera";
const CAMERA_SPAN: usize = 0x400;

fn current_camera(t: &Target, resolved: &Resolved) -> Option<usize> {
    let layout = &resolved.layout;
    let workspace = read_u64(t, resolved.character + layout.parent_off)? as usize;
    let is_camera = |inst: usize| {
        is_instance(t, inst) && class_name(t, layout.class_off, inst).as_deref() == Some(CAMERA_CLASS)
    };
    instance_fields(t, workspace, 0x800, is_camera)
        .first()
        .map(|&(_, camera)| camera)
        .or_else(|| children_of(t, layout.children_off?, workspace).into_iter().find(|&c| is_camera(c)))
}

pub fn camera_cframes(t: &Target, resolved: &Resolved) -> Option<(usize, Vec<(usize, RootPose)>)> {
    let camera = current_camera(t, resolved)?;
    let body = read_span(t, camera, CAMERA_SPAN);
    let found = (0..body.len().saturating_sub(47))
        .step_by(4)
        .filter_map(|off| parse_rot_first(&body[off..off + 48]).map(|pose| (off, pose)))
        .filter(|(_, pose)| pose.pos != [0.0; 3])
        .collect();
    Some((camera, found))
}

pub fn dump(t: &Target, addr: usize, span: usize) {
    let body = read_span(t, addr, span);
    for off in (0..body.len().saturating_sub(7)).step_by(8) {
        let v = u64_at(&body, off);
        let mut note = String::new();
        if is_heap_pointer(v) {
            if is_instance(t, v as usize) {
                note.push_str(" [instance]");
            }
            if let Some(text) = read_std_string(t, v as usize).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic() || c == ' ')) {
                note.push_str(&format!(" str=\"{text}\""));
            }
            let raw = read_span(t, v as usize, 24);
            let ascii: String = raw.iter().take_while(|&&c| (0x20..0x7f).contains(&c)).map(|&c| c as char).collect();
            if ascii.len() >= 3 {
                note.push_str(&format!(" raw=\"{ascii}\""));
            }
        }
        println!("+0x{off:03X}  {v:016X}{note}");
    }
}
