# Nepnote

A notes overlay for games on Windows. Press **Shift+F1** in a game on your list to open your notes on top of it, organized in folders, with checklists and a pinned card you can keep on screen while you play.

## Using it

- **Add a game:** open the game and press **Ctrl+Shift+F1**. Its notes open right away.
- **Open or close notes:** **Shift+F1** in that game. **Esc** also closes.
- **Folders, notes, checklists:** the **Folder**, **Note** and **List** buttons. **F2** renames, **Del** deletes (to the Recycle Bin), right-click for more, and drag notes into folders.
- **Folder style:** click a folder to pick its color and icon.
- **Pin:** shows the current note as a small card while you play. **Lock** keeps the overlay and card in place, and makes the card click-through so clicks go to your game.

Notes are plain `.txt` files in **Documents\Nepnote\<game>**, so you can open them without the app. The game list and settings are in **Documents\Nepnote\settings.json** (tray icon → *Edit game list and settings*).

## Anti-cheat safety

Nepnote is built to stay out of anti-cheat's way:

- **Hotkeys only when needed.** Shift+F1 is a standard Windows hotkey, registered only while a listed game is in front and released otherwise. There are no keyboard hooks.
- **Never touches the game.** Games are recognized from the Windows process list. No game process is opened, read, injected into or drawn inside.
- **Stands down around anti-cheat.** If Easy Anti-Cheat, BattlEye, FACEIT, EA Javelin, nProtect GameGuard, PunkBuster or ACE is running, or a game known to use kernel anti-cheat (Valorant, League, Genshin and other HoYoverse games, Fortnite, Apex, CS2 and more), all hotkeys are released and nothing is shown until it closes. The tray icon tooltip says why.
- **No admin rights.** It refuses to run as administrator.
- **Offline.** It has no network code; your notes never leave your PC.

You can add your own entries in `settings.json`:

- `"blocklist"`: games where Nepnote should never open.
- `"antiCheatProcesses"`: extra program names that should make Nepnote stand down.

No tool can guarantee a game's rules. Check each game's terms, and for competitive games keep notes outside the game.

## Building

GitHub Actions builds it on every push (see `.github/workflows/build.yml`). Download **Nepnote-windows** from the latest run: it has the portable `Nepnote.exe` and an installer.

To build locally: install Rust and Node.js, then run `npm install` and `npx tauri build`.
