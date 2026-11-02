#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Nepnote: a notes overlay for games.
//!
//! Anti-cheat safety rules this file follows:
//! - Shift+F1 is a normal Windows hotkey (RegisterHotKey), registered only while a
//!   listed game is in front, and released the rest of the time. No keyboard hooks.
//! - Games are recognised from the system process list; no game process is opened.
//! - If a known anti-cheat (or a game protected by one) is running, every hotkey is
//!   released and both windows are hidden until it closes.
//! - The overlay is an ordinary window on top: no injection, no drawing inside the
//!   game, no memory access, no simulated input. It refuses to run as admin.

mod ffi;

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const PIN_TITLE: &str = "Nepnote Pin";
const PIN_WIDTH: f64 = 300.0;

/// Anti-cheat programs. If any of these is running, Nepnote stands down.
const ANTI_CHEAT: &[(&str, &str)] = &[
    ("easyanticheat.exe", "Easy Anti-Cheat"),
    ("easyanticheat_eos.exe", "Easy Anti-Cheat"),
    ("easyanticheat_eos_setup.exe", "Easy Anti-Cheat"),
    ("beservice.exe", "BattlEye"),
    ("beservice_x64.exe", "BattlEye"),
    ("faceit.exe", "FACEIT"),
    ("faceitservice.exe", "FACEIT"),
    ("eaanticheat.gameservice.exe", "EA Javelin"),
    ("gamemon.des", "nProtect GameGuard"),
    ("gamemon.exe", "nProtect GameGuard"),
    ("gamemon64.exe", "nProtect GameGuard"),
    ("pnkbstra.exe", "PunkBuster"),
    ("pnkbstrb.exe", "PunkBuster"),
    ("ace-tray.exe", "ACE anti-cheat"),
    ("sguard64.exe", "ACE anti-cheat"),
];

/// Games whose anti-cheat runs as a driver (no separate program to spot), or that
/// we want to stay away from regardless. Nepnote stands down while they run.
const PROTECTED_GAMES: &[(&str, &str)] = &[
    ("valorant-win64-shipping.exe", "Riot Vanguard"),
    ("league of legends.exe", "Riot Vanguard"),
    ("genshinimpact.exe", "HoYoverse anti-cheat"),
    ("yuanshen.exe", "HoYoverse anti-cheat"),
    ("starrail.exe", "HoYoverse anti-cheat"),
    ("zenlesszonezero.exe", "HoYoverse anti-cheat"),
    ("bh3.exe", "HoYoverse anti-cheat"),
    ("fortniteclient-win64-shipping.exe", "Easy Anti-Cheat"),
    ("r5apex.exe", "Easy Anti-Cheat"),
    ("r5apex_dx12.exe", "Easy Anti-Cheat"),
    ("tslgame.exe", "BattlEye"),
    ("rainbowsix.exe", "BattlEye"),
    ("rainbowsix_be.exe", "BattlEye"),
    ("destiny2.exe", "BattlEye"),
    ("cod.exe", "Ricochet"),
    ("cs2.exe", "VAC / FACEIT"),
    ("dota2.exe", "VAC"),
    ("overwatch.exe", "Blizzard anti-cheat"),
    ("thefinals.exe", "Easy Anti-Cheat"),
    ("deadbydaylight-win64-shipping.exe", "Easy Anti-Cheat"),
    ("eldenring.exe", "Easy Anti-Cheat"),
    ("marvel-win64-shipping.exe", "NetEase anti-cheat"),
];

#[derive(Default)]
struct Shared {
    root: PathBuf,
    games: HashSet<String>,
    blocklist: HashSet<String>,
    extra_ac: Vec<String>,
    locked: bool,
    settings_mtime: Option<SystemTime>,
    toggle_on: bool,
    add_on: bool,
    paused: Option<String>,
    game_hwnd: isize,
    game_exe: String,
    pinned: HashMap<String, bool>,
    pin_hwnd: isize,
    pin_shown: bool,
    pin_locked_applied: Option<bool>,
    overlay_placed: bool,
    pin_placed: bool,
}

type St = Arc<Mutex<Shared>>;

#[derive(Serialize, Clone)]
struct GameInfo {
    exe: String,
    name: String,
}

#[derive(Serialize)]
struct Info {
    root: String,
    exe: String,
    name: String,
    paused: Option<String>,
}

#[derive(Serialize)]
struct Node {
    name: String,
    path: String,
    dir: bool,
    children: Vec<Node>,
}

fn toggle_key() -> Shortcut {
    Shortcut::new(Some(Modifiers::SHIFT), Code::F1)
}

fn add_key() -> Shortcut {
    Shortcut::new(Some(Modifiers::SHIFT | Modifiers::CONTROL), Code::F1)
}

// ---------------------------------------------------------------- names & paths

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .collect();
    cleaned.trim().trim_end_matches(['.', ' ']).to_string()
}

fn game_name(exe: &str) -> String {
    let base = if exe.to_lowercase().ends_with(".exe") { &exe[..exe.len() - 4] } else { exe };
    let s = sanitize(base);
    if s.is_empty() { "Game".to_string() } else { s }
}

fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut p = root.to_path_buf();
    for part in rel.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return Err("That path isn't allowed.".into());
        }
        p.push(part);
    }
    Ok(p)
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{base}{ext}"));
    if !first.exists() {
        return first;
    }
    let mut i = 2;
    loop {
        let p = dir.join(format!("{base} ({i}){ext}"));
        if !p.exists() {
            return p;
        }
        i += 1;
    }
}

/// Write to a temp file, then swap it in, so a crash never leaves half a note.
fn atomic_write(p: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = p.as_os_str().to_owned();
    tmp.push(".saving");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data)?;
    fs::rename(&tmp, p)
}

// ---------------------------------------------------------------- settings

fn settings_path(root: &Path) -> PathBuf {
    root.join("settings.json")
}

fn read_settings_value(root: &Path) -> Value {
    fs::read_to_string(settings_path(root))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}))
}

fn ensure_settings(root: &Path) {
    let mut v = read_settings_value(root);
    let obj = v.as_object_mut().unwrap();
    let mut changed = !settings_path(root).exists();
    for (k, d) in [
        ("games", json!([])),
        ("blocklist", json!([])),
        ("antiCheatProcesses", json!([])),
        ("opacity", json!(96)),
        ("locked", json!(false)),
    ] {
        if !obj.contains_key(k) {
            obj.insert(k.to_string(), d);
            changed = true;
        }
    }
    if changed {
        let _ = atomic_write(&settings_path(root), serde_json::to_string_pretty(&v).unwrap().as_bytes());
    }
}

fn str_set(v: &Value, key: &str) -> HashSet<String> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn reload_settings(s: &mut Shared) {
    let mtime = fs::metadata(settings_path(&s.root)).and_then(|m| m.modified()).ok();
    if mtime.is_some() && mtime == s.settings_mtime {
        return;
    }
    s.settings_mtime = mtime;
    let v = read_settings_value(&s.root);
    s.games = str_set(&v, "games");
    s.blocklist = str_set(&v, "blocklist");
    s.extra_ac = str_set(&v, "antiCheatProcesses").into_iter().collect();
    s.locked = v.get("locked").and_then(|b| b.as_bool()).unwrap_or(false);
}

fn is_protected_game(exe_lower: &str) -> bool {
    PROTECTED_GAMES.iter().any(|(p, _)| *p == exe_lower)
}

fn detect_pause(names: &HashSet<String>, extra: &[String]) -> Option<String> {
    for (p, label) in ANTI_CHEAT {
        if names.contains(*p) {
            return Some(format!("{label} is running"));
        }
    }
    for (p, label) in PROTECTED_GAMES {
        if names.contains(*p) {
            return Some(format!("a game protected by {label} is running"));
        }
    }
    for p in extra {
        if names.contains(p) {
            return Some(format!("{p} is running"));
        }
    }
    None
}

fn game_has_pin(root: &Path, exe: &str) -> bool {
    let p = root.join(game_name(exe)).join(".gamenotes.json");
    fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("pinned").and_then(|x| x.as_str()).map(|s| !s.is_empty()))
        .unwrap_or(false)
}

fn set_status(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

// ---------------------------------------------------------------- overlay & pin

fn pin_show(app: &AppHandle, show: bool) {
    let st = app.state::<St>().inner().clone();
    // Decide under the lock, act after releasing it (window calls may wait on the UI thread).
    let (hwnd, place, apply_lock, game_hwnd) = {
        let mut s = st.lock().unwrap();
        if s.pin_hwnd == 0 || !ffi::is_window(s.pin_hwnd) {
            s.pin_hwnd = ffi::find_window(PIN_TITLE);
            if s.pin_hwnd != 0 {
                ffi::make_no_activate(s.pin_hwnd);
            }
        }
        if s.pin_hwnd == 0 || s.pin_shown == show {
            return;
        }
        let place = show && !s.pin_placed;
        if place {
            s.pin_placed = true;
        }
        let apply = if show && s.pin_locked_applied != Some(s.locked) {
            s.pin_locked_applied = Some(s.locked);
            Some(s.locked)
        } else {
            None
        };
        s.pin_shown = show;
        (s.pin_hwnd, place, apply, s.game_hwnd)
    };
    if let Some(pin) = app.get_webview_window("pin") {
        if place {
            if let Some(r) = ffi::window_rect(game_hwnd) {
                let _ = pin.set_position(PhysicalPosition::new(r.left + 24, r.top + 80));
            }
        }
        if let Some(locked) = apply_lock {
            let _ = pin.set_ignore_cursor_events(locked);
        }
    }
    ffi::show_no_activate(hwnd, show);
    if show {
        let _ = app.emit("pin-refresh", ());
    }
}

fn hide_overlay_inner(app: &AppHandle) {
    let _ = app.emit("overlay-hidden", ());
    if let Some(ov) = app.get_webview_window("overlay") {
        let _ = ov.hide();
    }
    let hwnd = app.state::<St>().inner().lock().unwrap().game_hwnd;
    if ffi::is_window(hwnd) {
        ffi::set_foreground(hwnd);
    }
}

fn show_overlay_for(app: &AppHandle, hwnd: isize, exe: String) {
    let Some(ov) = app.get_webview_window("overlay") else { return };
    let st = app.state::<St>().inner().clone();
    let placed = {
        let mut s = st.lock().unwrap();
        s.game_hwnd = hwnd;
        s.game_exe = exe.clone();
        let p = s.overlay_placed;
        s.overlay_placed = true;
        p
    };
    if !placed {
        if let Some(r) = ffi::window_rect(hwnd) {
            let w = ov.outer_size().map(|s| s.width as i32).unwrap_or(780);
            let x = (r.right - w - 30).max(r.left);
            let _ = ov.set_position(PhysicalPosition::new(x, r.top + 60));
        }
    }
    pin_show(app, false);
    let _ = ov.show();
    let _ = ov.set_focus();
    let _ = app.emit("overlay-shown", GameInfo { name: game_name(&exe), exe });
}

/// Shift+F1
fn toggle_overlay(app: &AppHandle) {
    let (fg, pid) = ffi::foreground();
    if pid == std::process::id() {
        let visible = app.get_webview_window("overlay").and_then(|w| w.is_visible().ok()).unwrap_or(false);
        if visible {
            hide_overlay_inner(app);
        }
        return;
    }
    let exe = ffi::exe_of_pid(pid);
    let lower = exe.to_lowercase();
    let ok = {
        let st = app.state::<St>().inner().clone();
        let mut s = st.lock().unwrap();
        reload_settings(&mut s);
        s.paused.is_none() && !exe.is_empty() && s.games.contains(&lower) && !s.blocklist.contains(&lower)
    };
    if ok {
        show_overlay_for(app, fg, exe);
    }
}

/// Ctrl+Shift+F1: add the game in front to the list, then open its notes.
fn add_current_game(app: &AppHandle) {
    let (fg, pid) = ffi::foreground();
    if pid == std::process::id() {
        return;
    }
    let exe = ffi::exe_of_pid(pid);
    let lower = exe.to_lowercase();
    if exe.is_empty() || lower == "explorer.exe" || is_protected_game(&lower) {
        return;
    }
    let st = app.state::<St>().inner().clone();
    let root = {
        let s = st.lock().unwrap();
        if s.paused.is_some() || s.blocklist.contains(&lower) {
            return;
        }
        s.root.clone()
    };
    let mut v = read_settings_value(&root);
    let games = v.as_object_mut().unwrap().entry("games").or_insert_with(|| json!([]));
    if let Some(arr) = games.as_array_mut() {
        if !arr.iter().any(|x| x.as_str().map(|s| s.eq_ignore_ascii_case(&exe)).unwrap_or(false)) {
            arr.push(json!(exe));
        }
    }
    let _ = atomic_write(&settings_path(&root), serde_json::to_string_pretty(&v).unwrap().as_bytes());
    {
        let mut s = st.lock().unwrap();
        s.settings_mtime = None;
        reload_settings(&mut s);
    }
    let _ = app.emit("settings-changed", ());
    show_overlay_for(app, fg, exe);
}

// ---------------------------------------------------------------- the watcher

/// Runs a few times a second. Decides whether hotkeys should exist right now and
/// whether the pinned card should be on screen.
fn poll(app: &AppHandle) {
    let st = app.state::<St>().inner().clone();
    let procs = ffi::process_list();
    let names: HashSet<String> = procs.iter().map(|(_, n)| n.to_lowercase()).collect();
    let (fg, pid) = ffi::foreground();
    let ours = pid == std::process::id();
    let fg_exe = procs.iter().find(|(p, _)| *p == pid).map(|(_, n)| n.clone()).unwrap_or_default();
    let fg_lower = fg_exe.to_lowercase();
    let overlay_visible = app.get_webview_window("overlay").and_then(|w| w.is_visible().ok()).unwrap_or(false);

    let (want_toggle, want_add, want_pin, toggle_on, add_on, paused_change) = {
        let mut s = st.lock().unwrap();
        reload_settings(&mut s);
        let paused = detect_pause(&names, &s.extra_ac);
        let is_game = !fg_lower.is_empty() && s.games.contains(&fg_lower) && !s.blocklist.contains(&fg_lower);
        if is_game && fg != s.game_hwnd {
            s.game_hwnd = fg;
            s.game_exe = fg_exe.clone();
        }
        let pinned_here = if is_game {
            let root = s.root.clone();
            *s.pinned.entry(fg_lower.clone()).or_insert_with(|| game_has_pin(&root, &fg_exe))
        } else {
            false
        };
        let want_toggle = paused.is_none() && (ours || is_game);
        let want_add = paused.is_none() && !ours;
        let want_pin = paused.is_none() && !overlay_visible && is_game && pinned_here;
        let change = if paused != s.paused { Some(paused.clone()) } else { None };
        s.paused = paused;
        (want_toggle, want_add, want_pin, s.toggle_on, s.add_on, change)
    };

    // Register / release hotkeys outside the lock.
    let gs = app.global_shortcut();
    if want_toggle != toggle_on {
        let ok = if want_toggle { gs.register(toggle_key()).is_ok() } else { gs.unregister(toggle_key()).is_ok() };
        if ok || !want_toggle {
            st.lock().unwrap().toggle_on = want_toggle;
        }
    }
    if want_add != add_on {
        let ok = if want_add { gs.register(add_key()).is_ok() } else { gs.unregister(add_key()).is_ok() };
        if ok || !want_add {
            st.lock().unwrap().add_on = want_add;
        }
    }

    if let Some(p) = paused_change {
        match p {
            Some(reason) => {
                if overlay_visible {
                    hide_overlay_inner(app);
                }
                set_status(app, &format!("Nepnote is paused: {reason}"));
            }
            None => set_status(app, "Nepnote - Shift+F1 in a listed game"),
        }
        let _ = app.emit("paused-changed", ());
    }

    let shown = st.lock().unwrap().pin_shown;
    if want_pin != shown {
        pin_show(app, want_pin);
    }
}

// ---------------------------------------------------------------- commands

fn root_of(st: &tauri::State<'_, St>) -> PathBuf {
    st.lock().unwrap().root.clone()
}

#[tauri::command]
fn get_info(st: tauri::State<'_, St>) -> Info {
    let s = st.lock().unwrap();
    Info {
        root: s.root.to_string_lossy().to_string(),
        exe: s.game_exe.clone(),
        name: if s.game_exe.is_empty() { String::new() } else { game_name(&s.game_exe) },
        paused: s.paused.clone(),
    }
}

#[tauri::command]
fn read_text(st: tauri::State<'_, St>, rel: String) -> Result<String, String> {
    let p = resolve(&root_of(&st), &rel)?;
    match fs::read_to_string(&p) {
        Ok(t) => Ok(t.trim_start_matches('\u{feff}').replace("\r\n", "\n")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn write_text(st: tauri::State<'_, St>, rel: String, content: String) -> Result<(), String> {
    let p = resolve(&root_of(&st), &rel)?;
    let data = if p.extension().map(|e| e.eq_ignore_ascii_case("txt")).unwrap_or(false) {
        content.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        content
    };
    atomic_write(&p, data.as_bytes()).map_err(|e| e.to_string())
}

fn walk(root: &Path, dir: &Path, depth: u32) -> Vec<Node> {
    let mut out = Vec::new();
    if depth > 16 {
        return out;
    }
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            out.push(Node { children: walk(root, &path, depth + 1), path: rel_of(root, &path), name, dir: true });
        } else if name.to_lowercase().ends_with(".txt") {
            out.push(Node { path: rel_of(root, &path), name, dir: false, children: vec![] });
        }
    }
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

#[tauri::command]
fn list_tree(st: tauri::State<'_, St>, dir: String) -> Result<Vec<Node>, String> {
    let root = root_of(&st);
    let p = resolve(&root, &dir)?;
    fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    Ok(walk(&root, &p, 0))
}

#[tauri::command]
fn make_dir(st: tauri::State<'_, St>, parent: String, base: String) -> Result<String, String> {
    let root = root_of(&st);
    let dir = resolve(&root, &parent)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = sanitize(&base);
    let p = unique_path(&dir, if name.is_empty() { "New folder" } else { &name }, "");
    fs::create_dir(&p).map_err(|e| e.to_string())?;
    Ok(rel_of(&root, &p))
}

#[tauri::command]
fn make_note(st: tauri::State<'_, St>, parent: String, base: String, content: String) -> Result<String, String> {
    let root = root_of(&st);
    let dir = resolve(&root, &parent)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = sanitize(&base);
    let p = unique_path(&dir, if name.is_empty() { "New note" } else { &name }, ".txt");
    atomic_write(&p, content.replace('\n', "\r\n").as_bytes()).map_err(|e| e.to_string())?;
    Ok(rel_of(&root, &p))
}

#[tauri::command]
fn rename_item(st: tauri::State<'_, St>, rel: String, name: String) -> Result<String, String> {
    let root = root_of(&st);
    let p = resolve(&root, &rel)?;
    if rel.split('/').filter(|x| !x.is_empty()).count() < 2 {
        return Err("The game folder itself can't be renamed here.".into());
    }
    let mut clean = sanitize(&name);
    if clean.is_empty() {
        return Err("A name can't be empty.".into());
    }
    let is_dir = p.is_dir();
    if !is_dir && !clean.to_lowercase().ends_with(".txt") {
        clean.push_str(".txt");
    }
    let parent = p.parent().ok_or("Couldn't find the parent folder.")?;
    let target = parent.join(&clean);
    if target == p {
        return Ok(rel);
    }
    let same_ignoring_case = target.to_string_lossy().to_lowercase() == p.to_string_lossy().to_lowercase();
    if target.exists() && !same_ignoring_case {
        return Err("Something with that name is already in this folder.".into());
    }
    fs::rename(&p, &target).map_err(|e| e.to_string())?;
    Ok(rel_of(&root, &target))
}

#[tauri::command]
fn move_item(st: tauri::State<'_, St>, rel: String, dest: String) -> Result<String, String> {
    let root = root_of(&st);
    let src = resolve(&root, &rel)?;
    let dir = resolve(&root, &dest)?;
    if src.is_dir() && dir.starts_with(&src) {
        return Err("A folder can't go inside itself.".into());
    }
    if src.parent() == Some(dir.as_path()) {
        return Ok(rel);
    }
    let fname = src.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let target = if src.is_dir() {
        unique_path(&dir, &fname, "")
    } else {
        let stem = fname.strip_suffix(".txt").or_else(|| fname.strip_suffix(".TXT")).unwrap_or(&fname).to_string();
        unique_path(&dir, &stem, ".txt")
    };
    fs::rename(&src, &target).map_err(|e| e.to_string())?;
    Ok(rel_of(&root, &target))
}

#[tauri::command]
fn delete_item(st: tauri::State<'_, St>, rel: String) -> Result<(), String> {
    let root = root_of(&st);
    if rel.split('/').filter(|x| !x.is_empty()).count() < 2 {
        return Err("The game folder itself can't be deleted here.".into());
    }
    let p = resolve(&root, &rel)?;
    trash::delete(&p).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_folder(st: tauri::State<'_, St>, rel: String) -> Result<(), String> {
    let p = resolve(&root_of(&st), &rel)?;
    let target = if p.is_dir() { p } else { p.parent().map(|x| x.to_path_buf()).unwrap_or(p) };
    std::process::Command::new("explorer").arg(target).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn hide_overlay(app: AppHandle) {
    hide_overlay_inner(&app);
}

#[tauri::command]
fn set_pin_state(app: AppHandle, pinned: bool, locked: bool) {
    let st = app.state::<St>().inner().clone();
    {
        let mut s = st.lock().unwrap();
        s.locked = locked;
        let key = s.game_exe.to_lowercase();
        if !key.is_empty() {
            s.pinned.insert(key, pinned);
        }
        s.pin_locked_applied = Some(locked);
    }
    if let Some(pin) = app.get_webview_window("pin") {
        let _ = pin.set_ignore_cursor_events(locked);
    }
    if !pinned {
        pin_show(&app, false);
    }
    let _ = app.emit("pin-refresh", ());
}

#[tauri::command]
fn resize_pin(app: AppHandle, height: f64) {
    if let Some(pin) = app.get_webview_window("pin") {
        let _ = pin.set_size(LogicalSize::new(PIN_WIDTH, height.clamp(60.0, 460.0)));
    }
}

// ---------------------------------------------------------------- main

fn main() {
    if ffi::is_elevated() {
        ffi::message(
            "Nepnote is running as administrator.\n\nFor safety around game anti-cheat it only runs as a normal app. Close it and start it again normally (not \"Run as administrator\").",
        );
        return;
    }
    if ffi::already_running() {
        ffi::message("Nepnote is already running. Look for its icon in the system tray.");
        return;
    }

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if shortcut == &toggle_key() {
                        toggle_overlay(app);
                    } else if shortcut == &add_key() {
                        add_current_game(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let root = app
                .path()
                .document_dir()
                .map(|d| d.join("Nepnote"))
                .unwrap_or_else(|_| PathBuf::from("Nepnote"));
            let _ = fs::create_dir_all(&root);
            ensure_settings(&root);
            let st: St = Arc::new(Mutex::new(Shared { root: root.clone(), ..Default::default() }));
            app.manage(st);

            let open = MenuItem::with_id(app, "open", "Open notes folder", true, None::<&str>)?;
            let edit = MenuItem::with_id(app, "settings", "Edit game list and settings", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            // Off by default. If it was turned on, refresh the entry so it follows the exe if it moved.
            let startup_on = ffi::startup_enabled();
            if startup_on {
                ffi::set_startup(true);
            }
            let startup = CheckMenuItem::with_id(app, "startup", "Start with Windows", true, startup_on, None::<&str>)?;
            let startup_item = startup.clone();
            let quit = MenuItem::with_id(app, "quit", "Exit Nepnote", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &edit, &startup, &sep, &quit])?;
            let menu_root = root.clone();
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("Nepnote - Shift+F1 in a listed game")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => {
                        let _ = std::process::Command::new("explorer").arg(&menu_root).spawn();
                    }
                    "settings" => {
                        let _ = std::process::Command::new("notepad.exe").arg(settings_path(&menu_root)).spawn();
                    }
                    "startup" => {
                        let want = !ffi::startup_enabled();
                        ffi::set_startup(want);
                        let _ = startup_item.set_checked(ffi::startup_enabled());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(400));
                poll(&handle);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_info,
            read_text,
            write_text,
            list_tree,
            make_dir,
            make_note,
            rename_item,
            move_item,
            delete_item,
            open_folder,
            hide_overlay,
            set_pin_state,
            resize_pin
        ])
        .run(tauri::generate_context!())
        .expect("Nepnote failed to start");
}
