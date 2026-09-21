// The window. GDI into a back buffer, raw Win32, no dependencies.
//
// The plan view looks straight down: +X right, +Z down, so Roblox's forward is up.

use super::tracker::{self, Pose, Shared, Snapshot};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, OnceLock};

type Handle = *mut c_void;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    wnd_proc: unsafe extern "system" fn(Handle, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Size {
    cx: i32,
    cy: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[link(name = "user32")]
extern "system" {
    fn SetProcessDPIAware() -> i32;
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32, class: *const u16, title: *const u16, style: u32,
        x: i32, y: i32, w: i32, h: i32,
        parent: Handle, menu: Handle, instance: Handle, param: *mut c_void,
    ) -> Handle;
    fn DefWindowProcW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn SetTimer(hwnd: Handle, id: usize, millis: u32, callback: *const c_void) -> usize;
    fn InvalidateRect(hwnd: Handle, rect: *const Rect, erase: i32) -> i32;
    fn BeginPaint(hwnd: Handle, paint: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Handle, paint: *const PaintStruct) -> i32;
    fn GetClientRect(hwnd: Handle, rect: *mut Rect) -> i32;
    fn FillRect(hdc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn GetDC(hwnd: Handle) -> Handle;
    fn ReleaseDC(hwnd: Handle, hdc: Handle) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn CreateCompatibleBitmap(hdc: Handle, w: i32, h: i32) -> Handle;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn DeleteDC(hdc: Handle) -> i32;
    fn BitBlt(dst: Handle, x: i32, y: i32, w: i32, h: i32, src: Handle, sx: i32, sy: i32, rop: u32) -> i32;
    fn CreateSolidBrush(color: u32) -> Handle;
    fn CreatePen(style: i32, width: i32, color: u32) -> Handle;
    fn MoveToEx(hdc: Handle, x: i32, y: i32, previous: *mut Point) -> i32;
    fn LineTo(hdc: Handle, x: i32, y: i32) -> i32;
    fn Polyline(hdc: Handle, points: *const Point, count: i32) -> i32;
    fn Polygon(hdc: Handle, points: *const Point, count: i32) -> i32;
    fn Ellipse(hdc: Handle, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    fn SetBkMode(hdc: Handle, mode: i32) -> i32;
    fn SetTextColor(hdc: Handle, color: u32) -> u32;
    fn TextOutW(hdc: Handle, x: i32, y: i32, text: *const u16, len: i32) -> i32;
    fn GetTextExtentPoint32W(hdc: Handle, text: *const u16, len: i32, size: *mut Size) -> i32;
    fn CreateFontW(
        height: i32, width: i32, escapement: i32, orientation: i32, weight: i32,
        italic: u32, underline: u32, strike_out: u32, charset: u32, out_precision: u32,
        clip_precision: u32, quality: u32, pitch_and_family: u32, face: *const u16,
    ) -> Handle;
    fn GetDeviceCaps(hdc: Handle, index: i32) -> i32;
    fn IntersectClipRect(hdc: Handle, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    fn SelectClipRgn(hdc: Handle, region: Handle) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_TIMER: u32 = 0x0113;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const CW_USEDEFAULT: i32 = i32::MIN;
const SRCCOPY: u32 = 0x00CC_0020;
const TRANSPARENT: i32 = 1;
const PS_SOLID: i32 = 0;
const PS_DOT: i32 = 2;
const LOGPIXELSY: i32 = 90;
const IDC_ARROW: *const u16 = 32512 as *const u16;
const CLEARTYPE_QUALITY: u32 = 5;
const FIXED_PITCH: u32 = 1;
const DEFAULT_PITCH: u32 = 0;

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

const BACKGROUND: u32 = rgb(0x11, 0x12, 0x14);
const PANEL: u32 = rgb(0x18, 0x1a, 0x1d);
const RULE: u32 = rgb(0x2b, 0x2f, 0x34);
const GRID_MINOR: u32 = rgb(0x1c, 0x1f, 0x22);
const GRID_MAJOR: u32 = rgb(0x28, 0x2d, 0x32);
const TEXT: u32 = rgb(0xc9, 0xcc, 0xd0);
const TEXT_DIM: u32 = rgb(0x78, 0x80, 0x88);
const PLAYER: u32 = rgb(0x82, 0xaa, 0xc9);
const CAMERA: u32 = rgb(0xc8, 0x8c, 0x52);
const CAMERA_CONE: u32 = rgb(0x2c, 0x25, 0x1c);
const CAMERA_CONE_EDGE: u32 = rgb(0x63, 0x4c, 0x30);
const TRAIL: u32 = rgb(0x55, 0x78, 0x60);

const GRID_STUDS: f32 = 10.0;
const MAJOR_EVERY: i32 = 5;
const TRAIL_MAX: usize = 600;
const TRAIL_RESET_JUMP: f32 = 200.0;
const TABLE_ROWS: i32 = 3;
const NO_VALUE: &str = "\u{00b7}";
const SEPARATOR: &str = "   \u{00b7}   ";

struct View {
    pixels_per_stud: f32,
    trail: Vec<[f32; 2]>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static VIEW: Mutex<View> = Mutex::new(View { pixels_per_stud: 4.0, trail: Vec::new() });

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn system_scale() -> f32 {
    unsafe {
        let hdc = GetDC(null_mut());
        let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
        ReleaseDC(null_mut(), hdc);
        dpi as f32 / 96.0
    }
}

pub fn run() {
    let shared: Shared = Arc::new(Mutex::new(Snapshot { status: "Starting".into(), ..Default::default() }));
    let _ = SHARED.set(shared.clone());
    tracker::spawn(shared);

    unsafe {
        SetProcessDPIAware();
        let scale = system_scale();
        let instance = GetModuleHandleW(null());
        let class_name = wide("PxcRobloxPose");
        let class = WndClassExW {
            size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: window_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: null_mut(),
            cursor: LoadCursorW(null_mut(), IDC_ARROW),
            background: null_mut(),
            menu_name: null(),
            class_name: class_name.as_ptr(),
            icon_small: null_mut(),
        };
        RegisterClassExW(&class);
        let title = wide("Roblox Pose Reader");
        let hwnd = CreateWindowExW(
            0, class_name.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT, CW_USEDEFAULT, (460.0 * scale) as i32, (600.0 * scale) as i32,
            null_mut(), null_mut(), instance, null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        SetTimer(hwnd, 1, 33, null());
        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TIMER => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xffff) as u16 as i16;
            if let Ok(mut view) = VIEW.lock() {
                let factor = if delta > 0 { 1.2 } else { 1.0 / 1.2 };
                view.pixels_per_stud = (view.pixels_per_stud * factor).clamp(0.2, 40.0);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint(hwnd: Handle) {
    let mut ps: PaintStruct = std::mem::zeroed();
    let window_dc = BeginPaint(hwnd, &mut ps);
    let mut client = Rect::default();
    GetClientRect(hwnd, &mut client);
    let (w, h) = (client.right.max(1), client.bottom.max(1));

    let hdc = CreateCompatibleDC(window_dc);
    let bitmap = CreateCompatibleBitmap(window_dc, w, h);
    let old_bitmap = SelectObject(hdc, bitmap);

    let snapshot = SHARED.get().and_then(|s| s.lock().ok().map(|g| g.clone())).unwrap_or_default();
    let scale = GetDeviceCaps(hdc, LOGPIXELSY) as f32 / 96.0;
    let canvas = Canvas {
        hdc,
        scale,
        ui: create_font("Segoe UI", 12.0 * scale, DEFAULT_PITCH),
        mono: create_font("Consolas", 13.0 * scale, FIXED_PITCH),
    };
    SetBkMode(hdc, TRANSPARENT);
    canvas.fill(client, BACKGROUND);

    let pad = (9.0 * scale) as i32;
    let row = (18.0 * scale) as i32;
    let table_bottom = pad * 2 + row * TABLE_ROWS;
    let status_top = h - (23.0 * scale) as i32;

    canvas.fill(Rect { left: 0, top: 0, right: w, bottom: table_bottom }, PANEL);
    draw_table(&canvas, &snapshot, pad, row);
    canvas.rule(0, table_bottom, w);

    draw_plan(&canvas, &snapshot, Rect { left: 0, top: table_bottom + 1, right: w, bottom: status_top });

    canvas.fill(Rect { left: 0, top: status_top, right: w, bottom: h }, PANEL);
    canvas.rule(0, status_top, w);
    draw_status(&canvas, &snapshot, Rect { left: 0, top: status_top, right: w, bottom: h }, pad);

    BitBlt(window_dc, 0, 0, w, h, hdc, 0, 0, SRCCOPY);
    canvas.dispose();
    SelectObject(hdc, old_bitmap);
    DeleteObject(bitmap);
    DeleteDC(hdc);
    EndPaint(hwnd, &ps);
}

unsafe fn create_font(face: &str, pixels: f32, pitch: u32) -> Handle {
    let name = wide(face);
    CreateFontW(
        -(pixels as i32), 0, 0, 0, 400, 0, 0, 0, 0, 0, 0,
        CLEARTYPE_QUALITY, pitch, name.as_ptr(),
    )
}

struct Canvas {
    hdc: Handle,
    scale: f32,
    ui: Handle,
    mono: Handle,
}

impl Canvas {
    unsafe fn dispose(&self) {
        DeleteObject(self.ui);
        DeleteObject(self.mono);
    }

    fn px(&self, logical: f32) -> i32 {
        (logical * self.scale).round().max(1.0) as i32
    }

    unsafe fn fill(&self, rect: Rect, color: u32) {
        let brush = CreateSolidBrush(color);
        FillRect(self.hdc, &rect, brush);
        DeleteObject(brush);
    }

    unsafe fn rule(&self, left: i32, top: i32, right: i32) {
        self.fill(Rect { left, top, right, bottom: top + self.px(1.0) }, RULE);
    }

    unsafe fn with_pen(&self, style: i32, width: f32, color: u32, draw: impl FnOnce()) {
        let pen = CreatePen(style, self.px(width), color);
        let old = SelectObject(self.hdc, pen);
        draw();
        SelectObject(self.hdc, old);
        DeleteObject(pen);
    }

    unsafe fn line(&self, a: (i32, i32), b: (i32, i32)) {
        MoveToEx(self.hdc, a.0, a.1, null_mut());
        LineTo(self.hdc, b.0, b.1);
    }

    unsafe fn polygon(&self, points: &[Point], fill: u32, outline: u32) {
        let brush = CreateSolidBrush(fill);
        let old_brush = SelectObject(self.hdc, brush);
        self.with_pen(PS_SOLID, 1.0, outline, || {
            Polygon(self.hdc, points.as_ptr(), points.len() as i32);
        });
        SelectObject(self.hdc, old_brush);
        DeleteObject(brush);
    }

    unsafe fn ring(&self, center: (i32, i32), radius: f32, color: u32) {
        let r = self.px(radius);
        let brush = CreateSolidBrush(BACKGROUND);
        let old = SelectObject(self.hdc, brush);
        self.with_pen(PS_SOLID, 1.5, color, || {
            Ellipse(self.hdc, center.0 - r, center.1 - r, center.0 + r, center.1 + r);
        });
        SelectObject(self.hdc, old);
        DeleteObject(brush);
    }

    unsafe fn measure(&self, font: Handle, text: &str) -> i32 {
        let units: Vec<u16> = text.encode_utf16().collect();
        let old = SelectObject(self.hdc, font);
        let mut size = Size::default();
        GetTextExtentPoint32W(self.hdc, units.as_ptr(), units.len() as i32, &mut size);
        SelectObject(self.hdc, old);
        size.cx
    }

    unsafe fn text(&self, x: i32, y: i32, color: u32, font: Handle, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        let old = SelectObject(self.hdc, font);
        SetTextColor(self.hdc, color);
        TextOutW(self.hdc, x, y, units.as_ptr(), units.len() as i32);
        SelectObject(self.hdc, old);
    }

    unsafe fn text_right(&self, right: i32, y: i32, color: u32, font: Handle, text: &str) {
        self.text(right - self.measure(font, text), y, color, font, text);
    }
}

// Yaw matches Roblox's Orientation.Y: 0 degrees faces -Z, +90 faces -X.
fn yaw_pitch_degrees(look: [f32; 3]) -> (f32, f32) {
    ((-look[0]).atan2(-look[2]).to_degrees(), look[1].clamp(-1.0, 1.0).asin().to_degrees())
}

unsafe fn draw_table(canvas: &Canvas, snapshot: &Snapshot, pad: i32, row: i32) {
    let cell = canvas.measure(canvas.mono, "0").max(1);
    let label_width = canvas.px(46.0);
    let gap = canvas.px(13.0);
    // Widest values the columns must hold: -99999.99 studs and -179.9 degrees.
    let widths = [cell * 9, cell * 9, cell * 9, cell * 7, cell * 7];
    let mut edges = [0i32; 5];
    let mut x = pad + label_width;
    for (edge, width) in edges.iter_mut().zip(widths) {
        x += width;
        *edge = x;
        x += gap;
    }

    // The UI font's ascent is shorter than the mono font's; nudge it onto the baseline.
    let label_drop = canvas.px(1.0);
    for (column, heading) in edges.iter().zip(["X", "Y", "Z", "yaw", "pitch"]) {
        canvas.text_right(*column, pad + label_drop, TEXT_DIM, canvas.ui, heading);
    }

    let rows = [("player", PLAYER, snapshot.player), ("camera", CAMERA, snapshot.camera)];
    for (index, (label, color, pose)) in rows.into_iter().enumerate() {
        let y = pad + row * (index as i32 + 1);
        canvas.text(pad, y + label_drop, color, canvas.ui, label);
        let values = match pose {
            Some(Pose { pos, look, .. }) => {
                let (yaw, pitch) = yaw_pitch_degrees(look);
                [
                    format!("{:.2}", pos[0]),
                    format!("{:.2}", pos[1]),
                    format!("{:.2}", pos[2]),
                    format!("{yaw:.1}"),
                    format!("{pitch:.1}"),
                ]
            }
            None => std::array::from_fn(|_| NO_VALUE.to_string()),
        };
        let color = if pose.is_some() { TEXT } else { TEXT_DIM };
        for (column, value) in edges.iter().zip(&values) {
            canvas.text_right(*column, y, color, canvas.mono, value);
        }
    }
}

unsafe fn draw_status(canvas: &Canvas, snapshot: &Snapshot, area: Rect, pad: i32) {
    let y = area.top + (area.bottom - area.top - canvas.px(13.0)) / 2;
    let color = if snapshot.tracking { TEXT } else { TEXT_DIM };
    canvas.text(pad, y, color, canvas.ui, &snapshot.status);
    if !snapshot.tracking {
        return;
    }
    let mut parts = vec![snapshot.user.clone(), format!("pid {}", snapshot.pid)];
    if let (Some(player), Some(camera)) = (snapshot.player, snapshot.camera) {
        let range = (0..3).map(|i| (player.pos[i] - camera.pos[i]).powi(2)).sum::<f32>().sqrt();
        parts.push(format!("{range:.1} studs apart"));
    }
    canvas.text_right(area.right - pad, y, TEXT_DIM, canvas.ui, &parts.join(SEPARATOR));
}

// A map scale bar reads 1, 2 or 5 times a power of ten, never an arbitrary width.
fn scale_bar_studs(max_studs: f32) -> f32 {
    let decade = 10f32.powf(max_studs.max(1.0).log10().floor());
    [5.0, 2.0, 1.0]
        .into_iter()
        .map(|step| step * decade)
        .find(|candidate| *candidate <= max_studs)
        .unwrap_or(decade / 2.0)
}

unsafe fn draw_plan(canvas: &Canvas, snapshot: &Snapshot, area: Rect) {
    let Ok(mut view) = VIEW.lock() else { return };
    let hdc = canvas.hdc;
    IntersectClipRect(hdc, area.left, area.top, area.right, area.bottom);

    let zoom = view.pixels_per_stud * canvas.scale;
    let center = ((area.left + area.right) / 2, (area.top + area.bottom) / 2);
    let origin = snapshot.player.map_or([0.0, 0.0], |p| [p.pos[0], p.pos[2]]);
    let to_screen = |x: f32, z: f32| {
        (center.0 + ((x - origin[0]) * zoom) as i32, center.1 + ((z - origin[1]) * zoom) as i32)
    };

    // Grid lines sit on world coordinates, so they scroll as the player moves.
    let half_width = (area.right - area.left) as f32 / 2.0 / zoom;
    let half_height = (area.bottom - area.top) as f32 / 2.0 / zoom;
    let mut step = GRID_STUDS;
    while step * zoom < canvas.px(9.0) as f32 {
        step *= MAJOR_EVERY as f32;
    }
    for major in [false, true] {
        let color = if major { GRID_MAJOR } else { GRID_MINOR };
        canvas.with_pen(PS_SOLID, 1.0, color, || {
            let first = ((origin[0] - half_width) / step).floor() as i32;
            let last = ((origin[0] + half_width) / step).ceil() as i32;
            for index in (first..=last).filter(|i| (i % MAJOR_EVERY == 0) == major) {
                let x = to_screen(index as f32 * step, 0.0).0;
                canvas.line((x, area.top), (x, area.bottom));
            }
            let first = ((origin[1] - half_height) / step).floor() as i32;
            let last = ((origin[1] + half_height) / step).ceil() as i32;
            for index in (first..=last).filter(|i| (i % MAJOR_EVERY == 0) == major) {
                let y = to_screen(0.0, index as f32 * step).1;
                canvas.line((area.left, y), (area.right, y));
            }
        });
    }

    if !snapshot.tracking {
        view.trail.clear();
    }
    if let Some(player) = snapshot.player {
        let here = [player.pos[0], player.pos[2]];
        let step_from_last = |last: &[f32; 2]| (last[0] - here[0]).hypot(last[1] - here[1]);
        if view.trail.last().is_some_and(|last| step_from_last(last) > TRAIL_RESET_JUMP) {
            view.trail.clear();
        }
        if view.trail.last().map_or(true, |last| step_from_last(last) > 0.5) {
            view.trail.push(here);
            if view.trail.len() > TRAIL_MAX {
                view.trail.remove(0);
            }
        }
        let path: Vec<Point> = view
            .trail
            .iter()
            .map(|p| to_screen(p[0], p[1]))
            .chain(std::iter::once(center))
            .map(|(x, y)| Point { x, y })
            .collect();
        canvas.with_pen(PS_SOLID, 1.0, TRAIL, || {
            Polyline(hdc, path.as_ptr(), path.len() as i32);
        });

        if let Some(camera) = snapshot.camera {
            let eye = to_screen(camera.pos[0], camera.pos[2]);
            canvas.with_pen(PS_DOT, 1.0, RULE, || canvas.line(eye, center));
            view_cone(canvas, eye, camera.look, 52.0);
            canvas.ring(eye, 3.5, CAMERA);
        }
        heading_marker(canvas, center, player.look, 9.0);
    }

    draw_scale_bar(canvas, area, zoom);
    SelectClipRgn(hdc, null_mut());
}

// Screen axes match the world's: +X right, +Z down. `along` runs with the heading
fn plan_point(from: (i32, i32), heading: (f32, f32), along: f32, across: f32) -> Point {
    Point {
        x: from.0 + (heading.0 * along - heading.1 * across) as i32,
        y: from.1 + (heading.1 * along + heading.0 * across) as i32,
    }
}

// Horizontal heading of a look vector, and how much of it survives the projection:
fn flatten(look: [f32; 3]) -> Option<((f32, f32), f32)> {
    let flat = look[0].hypot(look[2]);
    (flat > 1.0e-3).then(|| ((look[0] / flat, look[2] / flat), flat.min(1.0)))
}

unsafe fn heading_marker(canvas: &Canvas, at: (i32, i32), look: [f32; 3], size: f32) {
    let Some((heading, _)) = flatten(look) else {
        return canvas.ring(at, size * 0.45, PLAYER);
    };
    let reach = size * canvas.scale;
    canvas.polygon(
        &[
            plan_point(at, heading, reach, 0.0),
            plan_point(at, heading, -reach * 0.8, reach * 0.66),
            plan_point(at, heading, -reach * 0.3, 0.0),
            plan_point(at, heading, -reach * 0.8, -reach * 0.66),
        ],
        PLAYER,
        PLAYER,
    );
}

unsafe fn view_cone(canvas: &Canvas, at: (i32, i32), look: [f32; 3], length: f32) {
    const HALF_ANGLE: f32 = 0.55;
    const ARC_STEPS: i32 = 8;
    let Some((heading, level)) = flatten(look) else { return };
    let reach = length * canvas.scale * level;
    let mut wedge = vec![Point { x: at.0, y: at.1 }];
    wedge.extend((0..=ARC_STEPS).map(|step| {
        let angle = -HALF_ANGLE + 2.0 * HALF_ANGLE * step as f32 / ARC_STEPS as f32;
        plan_point(at, heading, reach * angle.cos(), reach * angle.sin())
    }));
    canvas.polygon(&wedge, CAMERA_CONE, CAMERA_CONE_EDGE);
}

unsafe fn draw_scale_bar(canvas: &Canvas, area: Rect, zoom: f32) {
    let pad = canvas.px(10.0);
    let studs = scale_bar_studs(canvas.px(96.0) as f32 / zoom);
    let width = (studs * zoom) as i32;
    let y = area.bottom - pad - canvas.px(4.0);
    let left = area.left + pad;
    canvas.with_pen(PS_SOLID, 1.0, TEXT_DIM, || {
        canvas.line((left, y), (left + width, y));
        canvas.line((left, y - canvas.px(3.0)), (left, y));
        canvas.line((left + width, y - canvas.px(3.0)), (left + width, y));
    });
    let label = format!("{studs:.0} studs");
    canvas.text(left, y - canvas.px(21.0), TEXT_DIM, canvas.ui, &label);
}
