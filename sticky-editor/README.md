# Sticky Editor

Open any text file as a small floating note. Edits save themselves, and Markdown files render in place.

## Use

- `sticky-editor notes.md` opens a file. A path that doesn't exist yet is created on the first edit.
- After installing, right-click any file and choose **Open with Sticky Editor** (under "Show more options" on Windows 11).
- Or drop files onto any window, or press `Ctrl+O`.

Each file gets one window, which remembers its position, size and pin state.

| Shortcut | Action |
|---|---|
| `Ctrl+E` | Switch between edit and preview (Markdown files) |
| `Ctrl+T` | Toggle always on top |
| `Ctrl+S` | Save now |
| `Ctrl+O` | Open a file |
| `Ctrl+W` | Close |

Double-clicking the preview also returns to editing.

## How it treats your files

- **Saves about a second after you stop typing**, by writing a temp file and swapping it in, so a crash can't leave a half-written file.
- **Keeps the encoding, BOM and line endings** (UTF-8, UTF-16, GBK and other legacy encodings; CRLF, LF, CR). If you type a character the encoding can't represent, the save fails with a message rather than altering it.
- **Never overwrites changes made elsewhere.** An unedited note reloads by itself; with unsaved edits you choose between **Reload** and **Keep mine**.
- **Opens read-only** when the file is marked read-only, is larger than 5 MB, or its encoding can't be detected with certainty. Mixed line endings are unified on save, and a notice says so when the file opens.
- **Refuses** binary files and files over 20 MB.

Markdown preview runs no scripts and shows raw HTML as plain text. Images are not loaded: each appears as a link, so a file can't call out to a remote server. Links open in your browser.

## Build

Requires Node, Rust, and the WebView2 runtime (included with Windows 11).

```bash
npm install
npm run tauri dev
```

```bash
npm run tauri build
```

The installer is written to `src-tauri/target/release/bundle/nsis/`.
