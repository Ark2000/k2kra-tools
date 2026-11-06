import { invoke } from "@tauri-apps/api/core";
import { Menu } from "@tauri-apps/api/menu";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { ask, open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { render } from "./markdown.js";

const AUTOSAVE_MS = 1000;
const RETRY_MS = 3000;
const POLL_MS = 2000;
const MARKDOWN = /\.(md|markdown|mdown|mkd)$/i;
const COLORS = ["Yellow", "Pink", "Orange", "Green", "Blue", "Purple", "Gray"];
const OPACITIES = [100, 90, 80, 70, 60, 50, 40, 30];

const win = getCurrentWindow();
const $ = (id) => document.getElementById(id);
const editor = $("editor");
const preview = $("preview");

let doc = null;
let pinned = true;
let previewing = false;
let dirty = false;
let revision = 0; // bumped on every edit, so a save knows if it is still current
let saving = null; // the in-flight save, if any
let saveTimer = null;
let blocked = false; // a conflict or deletion awaits the user; autosave is off
let saveError = "";
let missing = false; // the last disk check could not find the file
let opacity = 100;
let color = "yellow";

// ---- rendering ------------------------------------------------------------

function showStatus() {
  if (!doc) return;
  const state = doc.readonly
    ? "Read-only"
    : saveError || (dirty ? "Editing…" : "Saved");
  $("status").textContent = `${doc.encoding} · ${doc.eol} · ${state}`;
  $("status").classList.toggle("error", Boolean(saveError));
  $("title").classList.toggle("dirty", dirty);
}

function setMode(wantPreview) {
  previewing = wantPreview;
  if (previewing) preview.innerHTML = render(editor.value);
  preview.hidden = !previewing;
  editor.hidden = previewing;
  $("toggle-preview").classList.toggle("active", previewing);
  if (!previewing) editor.focus();
}

function showBanner(text, actions = []) {
  $("banner-text").textContent = text;
  $("banner-actions").replaceChildren(
    ...actions.map(([label, run]) => {
      const button = document.createElement("button");
      button.textContent = label;
      button.addEventListener("click", run);
      return button;
    }),
  );
  $("banner").hidden = false;
}

function hideBanner() {
  $("banner").hidden = true;
}

function showMessage(text) {
  $("message").textContent = text;
  $("message").hidden = false;
  editor.hidden = true;
  preview.hidden = true;
}

function applyOpacity(percent) {
  opacity = percent;
  document.documentElement.style.setProperty("--alpha", percent / 100);
}

function changeOpacity(percent) {
  const clamped = Math.min(100, Math.max(30, percent));
  applyOpacity(clamped);
  invoke("set_opacity", { opacity: clamped / 100 });
}

function applyColor(name) {
  color = name;
  document.documentElement.dataset.color = name;
}

function changeColor(name) {
  applyColor(name);
  invoke("set_color", { color: name });
}

function fileLabel(path) {
  const parts = path.split(/[\\/]/);
  return { name: parts.at(-1), folder: parts.at(-2) ?? "" };
}

function showWelcome(recent) {
  const message = $("message");
  const hint = document.createElement("p");
  hint.textContent = "Drop a file here, or press Ctrl+O";
  const list = document.createElement("div");
  list.id = "recent";
  for (const path of recent) {
    const button = document.createElement("button");
    button.textContent = fileLabel(path).name;
    button.title = path;
    button.addEventListener("click", () => invoke("open_files", { paths: [path] }));
    list.append(button);
  }
  message.replaceChildren(hint, list);
  message.hidden = false;
}

function applyTheme(theme) {
  document.documentElement.dataset.theme =
    theme ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
}

// ---- saving ---------------------------------------------------------------

function scheduleSave(delay = AUTOSAVE_MS) {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, delay);
}

async function writeOut(force) {
  const sent = revision;
  try {
    await invoke("doc_save", { text: editor.value, force });
    if (sent === revision) dirty = false;
    saveError = "";
  } catch (error) {
    if (error.kind === "conflict") {
      showConflict();
    } else {
      saveError = error.message;
      // An unencodable character needs an edit, not a retry.
      if (error.kind === "io") scheduleSave(RETRY_MS);
    }
  }
  showStatus();
}

/** Resolves true once everything typed so far is on disk. */
async function save({ force = false } = {}) {
  clearTimeout(saveTimer);
  if (saving) await saving;
  if (!doc || doc.readonly || !dirty || (blocked && !force)) return !dirty;
  saving = writeOut(force).finally(() => (saving = null));
  await saving;
  if (dirty && !saveError && !blocked) scheduleSave();
  return !dirty;
}

function forceSave() {
  blocked = false;
  hideBanner();
  dirty = true;
  return save({ force: true });
}

// ---- files changing underneath us ----------------------------------------

async function loadDoc() {
  doc = await invoke("doc_load");
  if (!doc) return;
  const { selectionStart, selectionEnd, scrollTop } = editor;
  const previousScroll = preview.scrollTop;
  editor.value = doc.text;
  editor.setSelectionRange(selectionStart, selectionEnd);
  editor.scrollTop = scrollTop;
  editor.readOnly = doc.readonly;
  doc.text = null;
  dirty = false;
  blocked = false;
  saveError = "";
  hideBanner();
  if (doc.notice) showBanner(doc.notice);
  if (previewing) setMode(true);
  preview.scrollTop = previousScroll;
  showStatus();
}

function showConflict() {
  blocked = true;
  showBanner("Changed on disk by another program.", [
    ["Reload", loadDoc],
    ["Keep mine", forceSave],
  ]);
}

function showDeleted() {
  blocked = true;
  showBanner("This file was deleted.", [
    ["Save again", forceSave],
    ["Close", () => win.close()],
  ]);
}

function resume() {
  blocked = false;
  hideBanner();
  if (doc.notice) showBanner(doc.notice);
  if (dirty) scheduleSave();
}

async function checkDisk() {
  if (!doc || saving) return;
  const result = await invoke("doc_check").catch(() => "unknown");
  if (saving) return;
  const wasMissing = missing;
  missing = result === "deleted";
  if (result === "same" && blocked) resume();
  if (blocked) return;
  // Sync clients can replace a file by deleting it first, so one miss proves nothing.
  if (missing && wasMissing) showDeleted();
  else if (result === "changed") dirty ? showConflict() : await loadDoc();
}

// ---- commands -------------------------------------------------------------

const commands = {
  togglePreview() {
    if (doc && MARKDOWN.test(doc.name)) setMode(!previewing);
  },
  async togglePin() {
    pinned = !pinned;
    $("toggle-pin").classList.toggle("active", pinned);
    await invoke("set_pinned", { pinned });
  },
  async toggleTheme() {
    const theme = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    applyTheme(theme);
    await invoke("set_theme", { theme });
  },
  async open() {
    const picked = await openDialog({ multiple: true });
    if (picked) await invoke("open_files", { paths: picked });
  },
  async menu() {
    const recent = await invoke("recent");
    const recentItems = recent.map((path) => {
      const { name, folder } = fileLabel(path);
      return {
        text: `${name}  —  ${folder}`.replaceAll("&", "&&"), // "&" marks a mnemonic
        action: () => invoke("open_files", { paths: [path] }),
      };
    });
    const noteItems = !doc
      ? []
      : [
          {
            text: "Color",
            items: COLORS.map((name) => ({
              text: name,
              checked: name.toLowerCase() === color,
              action: () => changeColor(name.toLowerCase()),
            })),
          },
          {
            text: "Opacity",
            items: OPACITIES.map((percent) => ({
              text: `${percent}%`,
              checked: percent === opacity,
              action: () => changeOpacity(percent),
            })),
          },
        ];
    const autostart = await isEnabled();
    const menu = await Menu.new({
      items: [
        { text: "Open…", action: commands.open },
        {
          text: "Recent",
          enabled: recent.length > 0,
          items: [
            ...recentItems,
            { item: "Separator" },
            { text: "Clear list", action: () => invoke("clear_recent") },
          ],
        },
        { item: "Separator" },
        ...noteItems,
        {
          text: "Dark theme",
          checked: document.documentElement.dataset.theme === "dark",
          action: commands.toggleTheme,
        },
        {
          text: "Reopen notes at sign-in",
          checked: autostart,
          action: () => (autostart ? disable() : enable()),
        },
      ],
    });
    await menu.popup();
  },
};

const shortcuts = {
  s: () => save(),
  e: commands.togglePreview,
  o: commands.open,
  t: commands.togglePin,
  w: () => win.close(),
  r: () => {}, // a page reload would drop the document
  p: () => {},
};

document.addEventListener("keydown", (event) => {
  if (event.key === "F5") return event.preventDefault();
  const action = event.ctrlKey && !event.altKey && shortcuts[event.key.toLowerCase()];
  if (!action) return;
  event.preventDefault();
  action();
});

editor.addEventListener("input", () => {
  revision += 1;
  dirty = true;
  saveError = "";
  showStatus();
  if (!blocked) scheduleSave();
});

editor.addEventListener("keydown", (event) => {
  if (event.key !== "Tab" || event.ctrlKey) return;
  event.preventDefault();
  document.execCommand("insertText", false, "\t"); // keeps native undo intact
});

preview.addEventListener("dblclick", () => setMode(false));

preview.addEventListener("click", (event) => {
  const link = event.target.closest("a[href]");
  if (!link) return;
  event.preventDefault();
  const href = link.getAttribute("href");
  if (/^(https?:|mailto:)/i.test(href)) openUrl(href);
});

$("toggle-preview").addEventListener("click", commands.togglePreview);
$("toggle-pin").addEventListener("click", commands.togglePin);
$("menu").addEventListener("click", commands.menu);
$("minimize").addEventListener("click", () => win.minimize());
$("close").addEventListener("click", () => win.close());

document.addEventListener(
  "wheel",
  (event) => {
    if (!event.ctrlKey || !doc) return;
    event.preventDefault();
    changeOpacity(opacity + (event.deltaY < 0 ? 10 : -10));
  },
  { passive: false },
);

// ---- startup --------------------------------------------------------------

async function start() {
  const prefs = await invoke("prefs");
  pinned = prefs.pinned;
  applyTheme(prefs.theme);
  applyOpacity(Math.round(prefs.opacity * 100));
  applyColor(prefs.color);
  $("toggle-pin").classList.toggle("active", pinned);

  try {
    await loadDoc();
    if (doc) {
      $("title").textContent = doc.name;
      $("title").title = doc.path;
      const markdown = MARKDOWN.test(doc.name);
      $("toggle-preview").hidden = !markdown;
      setMode(markdown && editor.value.length > 0);
    } else {
      $("title").textContent = "Sticky Editor";
      showWelcome(await invoke("recent"));
    }
  } catch (error) {
    $("title").textContent = "Cannot open file";
    showMessage(String(error));
  }

  await win.show();
  await win.setFocus();
  if (doc && !previewing) editor.focus();
}

await win.onCloseRequested(async (event) => {
  const safe = await save();
  if (safe) return;
  const leave = await ask("Your latest changes could not be saved. Close anyway?", {
    title: "Sticky Editor",
    kind: "warning",
  });
  if (!leave) event.preventDefault();
});

await getCurrentWebview().onDragDropEvent(({ payload }) => {
  if (payload.type === "drop") invoke("open_files", { paths: payload.paths });
});

window.addEventListener("focus", checkDisk);
setInterval(checkDisk, POLL_MS);

await start();
