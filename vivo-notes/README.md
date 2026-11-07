# vivo-notes

Pull every note from vivo Office Suite (原子笔记) to this folder as Markdown, with a full raw JSON backup.

It talks to the Office Suite service on `127.0.0.1` directly. It does **not** need the `office-suite-notes` Skill — the Skill and this tool both just wrap the same local API.

## Use

Double-click `sync-now.cmd`, or run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\sync-notes.ps1
```

Office Suite must be running and signed in, with notes already synced to the PC. The token is read from `%USERPROFILE%\.notes-cli.conf`; set or refresh it with:

```powershell
notes config --token=<token>
```

## What it writes

Everything lands in `data/`:

```
data\
├── notes\             one .md per note, grouped by the note's folders
├── assets\<id>\       images referenced by notes
├── notes.json         full raw export (includes HTML bodies)
├── index.md           index sorted by update time
├── sync-state.json    incremental sync state
└── sync.log           log
```

- Titled notes are named after the title. Untitled notes use the first line of their body.
- The `[xxxxxxxx]` suffix is the first 8 characters of the note ID, so names never collide.
- Each `.md` starts with YAML metadata: `id`, `title`, `folder`, `created`, `updated`.
- After the first run only changed notes are downloaded.

## Options

| Option | Description |
| --- | --- |
| `-Full` | Ignore the incremental state and download every note again |
| `-Prune` | Also delete local `.md` files for notes removed from the cloud (default: keep them) |
| `-Quiet` | No console output, log only (used by the scheduled task) |
| `-OutDir <path>` | Write somewhere else instead of `data/` |

## Automatic sync

```powershell
# Register: sync at sign-in and every 30 minutes
powershell -NoProfile -ExecutionPolicy Bypass -File .\install-task.ps1

# Choose another interval
powershell -NoProfile -ExecutionPolicy Bypass -File .\install-task.ps1 -IntervalMinutes 15

# Remove the task
powershell -NoProfile -ExecutionPolicy Bypass -File .\install-task.ps1 -Remove
```

Task name: `vivo-notes-sync`. It runs only while you are signed in, because the Office Suite service runs in your session.

The task starts through `sync-hidden.vbs` (`wscript.exe`), so no console window is created and focus is never taken. `powershell.exe -WindowStyle Hidden` is not enough: it still flashes a console for a moment, which can interrupt a fullscreen game. The script also skips the run entirely while Apex Legends (`r5apex.exe` / `r5apex_dx12.exe`) is running, and lowers its own priority.

## Notes

- The service listens on the loopback interface only; notes never leave the machine.
- Notes deleted in the cloud are only reported in the log by default. Add `-Prune` to mirror deletions.
- If a run fails, check `data\sync.log` first. The usual causes are Office Suite not running or an expired token.
