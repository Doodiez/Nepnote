//! Tiny, read-only Windows calls. Nothing here opens, reads or touches another
//! process: we only ask Windows which window is in front and which programs are
//! running (the same list Task Manager shows).

#![allow(non_snake_case)]

use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
struct ProcessEntry32W {
    dw_size: u32,
    cnt_usage: u32,
    th32_process_id: u32,
    th32_default_heap_id: usize,
    th32_module_id: u32,
    cnt_threads: u32,
    th32_parent_process_id: u32,
    pc_pri_class_base: i32,
    dw_flags: u32,
    sz_exe_file: [u16; 260],
}

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
    fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn CreateMutexW(attrs: *const c_void, owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
}

#[link(name = "shell32")]
extern "system" {
    fn IsUserAnAdmin() -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// (window handle, process id) of the window the user is looking at.
pub fn foreground() -> (isize, u32) {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        if hwnd != 0 {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        (hwnd, pid)
    }
}

/// Every running program as (process id, exe name). Uses the system snapshot,
/// so no game process is ever opened.
pub fn process_list() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(0x0000_0002, 0); // TH32CS_SNAPPROCESS
        if snap == -1 || snap == 0 {
            return out;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let len = entry.sz_exe_file.iter().position(|&c| c == 0).unwrap_or(260);
                out.push((
                    entry.th32_process_id,
                    String::from_utf16_lossy(&entry.sz_exe_file[..len]),
                ));
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

pub fn exe_of_pid(pid: u32) -> String {
    process_list()
        .into_iter()
        .find(|(p, _)| *p == pid)
        .map(|(_, n)| n)
        .unwrap_or_default()
}

pub fn window_rect(hwnd: isize) -> Option<Rect> {
    let mut r = Rect::default();
    unsafe {
        if hwnd != 0 && GetWindowRect(hwnd, &mut r) != 0 {
            Some(r)
        } else {
            None
        }
    }
}

pub fn is_window(hwnd: isize) -> bool {
    unsafe { hwnd != 0 && IsWindow(hwnd) != 0 }
}

pub fn set_foreground(hwnd: isize) {
    unsafe {
        SetForegroundWindow(hwnd);
    }
}

pub fn find_window(title: &str) -> isize {
    let t = wide(title);
    unsafe { FindWindowW(std::ptr::null(), t.as_ptr()) }
}

/// Make a window that never takes keyboard focus (the pinned card), so the game
/// keeps focus while it is on screen.
pub fn make_no_activate(hwnd: isize) {
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
    }
}

pub fn show_no_activate(hwnd: isize, show: bool) {
    unsafe {
        ShowWindow(hwnd, if show { 4 } else { 0 }); // SW_SHOWNOACTIVATE / SW_HIDE
    }
}

pub fn message(text: &str) {
    let t = wide(text);
    let c = wide("Nepnote");
    unsafe {
        MessageBoxW(0, t.as_ptr(), c.as_ptr(), 0x30); // MB_ICONWARNING
    }
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// True if another copy of Nepnote is already running.
pub fn already_running() -> bool {
    let name = wide("Nepnote.SingleInstance.Doodiez");
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        h != 0 && GetLastError() == 183 // ERROR_ALREADY_EXISTS
    }
}

// ---------------------------------------------------------------- start with Windows
// A per-user "Run" entry, the standard way apps start at sign-in. Off unless the
// user turns it on from the tray menu.

#[link(name = "advapi32")]
extern "system" {
    fn RegSetKeyValueW(key: isize, subkey: *const u16, value: *const u16, kind: u32, data: *const c_void, len: u32) -> i32;
    fn RegDeleteKeyValueW(key: isize, subkey: *const u16, value: *const u16) -> i32;
    fn RegGetValueW(key: isize, subkey: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Nepnote";

pub fn startup_enabled() -> bool {
    let sub = wide(RUN_KEY);
    let val = wide(RUN_VALUE);
    let mut len: u32 = 0;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            val.as_ptr(),
            0x0000_0002, // RRF_RT_REG_SZ
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        ) == 0
    }
}

pub fn set_startup(on: bool) -> bool {
    let sub = wide(RUN_KEY);
    let val = wide(RUN_VALUE);
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return false };
            let cmd = wide(&format!("\"{}\"", exe.to_string_lossy()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                val.as_ptr(),
                1, // REG_SZ
                cmd.as_ptr() as *const c_void,
                (cmd.len() * 2) as u32,
            ) == 0
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), val.as_ptr());
            r == 0 || r == 2 // removed, or wasn't there
        }
    }
}
