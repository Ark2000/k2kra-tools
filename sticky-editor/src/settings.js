import { invoke } from "@tauri-apps/api/core";
import { COLORS } from "./visuals.js";

const $ = (id) => document.getElementById(id);

let profiles = [];
let defaultId = "";
let selected = "";

const current = () => profiles.find((profile) => profile.id === selected);

function showList() {
  $("profiles").replaceChildren(
    ...profiles.map(({ id, name }) => {
      const button = document.createElement("button");
      button.type = "button";
      button.role = "option";
      button.textContent = name || "Untitled";
      button.setAttribute("aria-selected", id === selected);
      button.addEventListener("click", () => select(id));
      return button;
    }),
  );
  $("remove").disabled = profiles.length < 2;
}

function showForm() {
  const profile = current();
  const percent = Math.round(profile.opacity * 100);
  $("name").value = profile.name;
  $("theme").value = profile.theme ?? "";
  $("opacity").value = percent;
  $("opacity-value").textContent = `${percent}%`;
  $("glass").checked = profile.glass;
  $("default").checked = profile.id === defaultId;
  $("default").disabled = profile.id === defaultId; // something must stay the default
  for (const button of $("colors").children) {
    button.setAttribute("aria-checked", button.dataset.color === profile.color);
  }
}

function select(id) {
  selected = id;
  showList();
  showForm();
}

function change(update) {
  update(current());
  invoke("save_profile", { profile: current() });
  showList();
  showForm();
}

$("colors").append(
  ...COLORS.map(({ name, swatch }) => {
    const button = document.createElement("button");
    button.type = "button";
    button.role = "radio";
    button.title = name;
    button.setAttribute("aria-label", name);
    button.dataset.color = name.toLowerCase();
    button.style.background = swatch;
    button.addEventListener("click", () => change((p) => (p.color = button.dataset.color)));
    return button;
  }),
);

$("form").addEventListener("submit", (event) => event.preventDefault());
$("name").addEventListener("input", (event) => change((p) => (p.name = event.target.value)));
$("theme").addEventListener("change", (event) => {
  change((p) => (p.theme = event.target.value || null));
});
$("opacity").addEventListener("input", (event) => {
  change((p) => (p.opacity = Number(event.target.value) / 100));
});
$("glass").addEventListener("change", (event) => change((p) => (p.glass = event.target.checked)));
$("default").addEventListener("change", async () => {
  defaultId = selected;
  await invoke("set_default_profile", { id: selected });
  showForm();
});

$("add").addEventListener("click", async () => {
  const profile = { ...current(), id: crypto.randomUUID(), name: `${current().name} copy` };
  profiles.push(profile);
  await invoke("save_profile", { profile });
  select(profile.id);
  $("name").select();
});

$("remove").addEventListener("click", async () => {
  await invoke("delete_profile", { id: selected });
  await load();
});

async function load() {
  ({ profiles, defaultProfile: defaultId } = await invoke("profiles"));
  select(profiles.some((profile) => profile.id === selected) ? selected : profiles[0].id);
}

await load();
