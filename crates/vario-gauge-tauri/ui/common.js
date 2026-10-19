// What the gauge and the main window share: the language, the knobs, the dragging.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

let FRENCH = null;
/** The label in the interface's language: French when `FR` (the page's) has it. */
const t = (text) => (FRENCH && FRENCH[text]) || text;
const $ = (id) => document.getElementById(id);
const percent = (value) => value < 0 ? "—" : Math.round(value) + " %";
const rad = (deg) => deg * Math.PI / 180;
const svg = (tag, attrs) => {
  const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
  return node;
};
const escape = (text) => String(text).replace(/[&<>"]/g,
  (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** Load the language, then word every `data-t` element. */
async function translate(french) {
  if ((await invoke("ui_lang")) === "fr") FRENCH = french;
  for (const node of document.querySelectorAll("[data-t]")) node.textContent = t(node.dataset.t);
}

// A small knob: an amber arc over 270 degrees, the value inside, the caption below.
function knobArc(from, to) {
  const r = 22, p = (deg) => [30 + r * Math.sin(rad(deg)), 30 - r * Math.cos(rad(deg))];
  const [x0, y0] = p(from), [x1, y1] = p(to);
  return `M${x0} ${y0} A${r} ${r} 0 ${to - from > 180 ? 1 : 0} 1 ${x1} ${y1}`;
}
/** `left` is in pixels, or any CSS length (`calc(50% - 66px)` centres it on its card). */
function knob(parent, left, top, caption) {
  const node = document.createElement("div");
  node.className = "abs knob";
  node.style.cssText = `left:${typeof left === "number" ? left + "px" : left};top:${top}px`;
  node.innerHTML = `<svg class="abs" style="left:0;top:0" width="60" height="60" viewBox="0 0 60 60">
      <path d="${knobArc(-135, 135)}" stroke="var(--unlit)" stroke-width="4" fill="none"/>
      <path class="value" stroke="var(--amber)" stroke-width="4" fill="none"/></svg>
    <div class="abs cap"></div>
    <div class="abs caption" style="left:0;top:62px;width:60px;text-align:center">${escape(caption)}</div>`;
  parent.append(node);
  return node;
}
function setKnob(node, value) {
  const share = Math.min(Math.max(value, 0), 100) / 100;
  node.querySelector(".value").setAttribute("d", value > 0.5 ? knobArc(-135, -135 + 270 * share) : "");
  node.querySelector(".cap").textContent = percent(value);
}

// The window is dragged from anywhere but its buttons and keys.
document.addEventListener("mousedown", (event) => {
  if (event.button === 0 && !event.target.closest(".click, button, input, label, .list"))
    getCurrentWindow().startDragging();
});

// The pages' errors go to the program's output, where they can be read.
const report = (message) => invoke("js_error", { page: location.pathname, message: String(message) });
window.addEventListener("error", (event) => report(`${event.message} (${event.filename}:${event.lineno})`));
window.addEventListener("unhandledrejection", (event) => report(event.reason));
