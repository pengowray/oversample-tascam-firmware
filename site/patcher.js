// Applies a patch made by tools/make_patch_data.py to TASCAM's V2.03 update file, in the browser.
// The file is checked by SHA-256 before and after; nothing is uploaded.
"use strict";

const $ = (id) => document.getElementById(id);
let patch = null;

async function loadPatch() {
  const latest = await (await fetch("patches/latest.json")).json();
  const folder = `patches/${latest.folder}/`;
  const manifest = await (await fetch(folder + "manifest.json")).json();
  const changes = new Uint8Array(await (await fetch(folder + "changes.bin")).arrayBuffer());
  patch = { manifest, changes };
  $("build").textContent = `B${manifest.build}`;
  $("original-name").textContent = `${manifest.original.name} (V2.03 build 217), ${manifest.original.size.toLocaleString()} bytes`;
  $("original-sha").textContent = manifest.original.sha256;
  $("patched-name").textContent = `${manifest.patched.name}, ${manifest.patched.size.toLocaleString()} bytes`;
  $("patched-sha").textContent = manifest.patched.sha256;
}

async function sha256(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** The .203 file inside a zip, by reading the zip's central directory. */
async function fromZip(zip) {
  const view = new DataView(zip.buffer, zip.byteOffset, zip.byteLength);
  let end = zip.length - 22;
  while (end >= 0 && view.getUint32(end, true) !== 0x06054b50) end--;
  if (end < 0) throw new Error("This zip file could not be read.");
  const count = view.getUint16(end + 10, true);
  let at = view.getUint32(end + 16, true);
  for (let i = 0; i < count; i++) {
    const method = view.getUint16(at + 10, true);
    const compressed = view.getUint32(at + 20, true);
    const nameLength = view.getUint16(at + 28, true);
    const extra = view.getUint16(at + 30, true);
    const comment = view.getUint16(at + 32, true);
    const local = view.getUint32(at + 42, true);
    const name = new TextDecoder().decode(zip.subarray(at + 46, at + 46 + nameLength));
    at += 46 + nameLength + extra + comment;
    if (!name.toLowerCase().endsWith(".203")) continue;
    const dataAt = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true);
    const data = zip.subarray(dataAt, dataAt + compressed);
    if (method === 0) return { name, bytes: data };
    if (method !== 8) throw new Error(`The zip uses a compression method this page can't read (${method}).`);
    const stream = new Blob([data]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
    return { name, bytes: new Uint8Array(await new Response(stream).arrayBuffer()) };
  }
  throw new Error("The zip has no .203 update file in it.");
}

/** The patched file: the original with each run of changed bytes written over it. */
function applyPatch(original) {
  const { manifest, changes } = patch;
  const out = new Uint8Array(manifest.patched.size);
  out.set(original.subarray(0, Math.min(original.length, out.length)));
  const view = new DataView(changes.buffer, changes.byteOffset, changes.byteLength);
  for (let at = 0; at < changes.length; ) {
    const offset = view.getUint32(at, true);
    const length = view.getUint32(at + 4, true);
    out.set(changes.subarray(at + 8, at + 8 + length), offset);
    at += 8 + length;
  }
  return out;
}

function check(label, ok, detail) {
  const item = document.createElement("li");
  item.className = ok ? "ok" : "fail";
  item.innerHTML = '<span class="label"></span><span class="detail"></span>';
  item.querySelector(".label").textContent = label;
  item.querySelector(".detail").textContent = detail;
  $("checks").append(item);
  return ok;
}

async function handle(file) {
  $("checks").replaceChildren();
  const link = $("download");
  link.hidden = true;
  if (link.href) URL.revokeObjectURL(link.href);
  try {
    if (!patch) await loadPatch();
    const { manifest } = patch;
    let bytes = new Uint8Array(await file.arrayBuffer());
    let name = file.name;
    if (bytes[0] === 0x50 && bytes[1] === 0x4b) ({ name, bytes } = await fromZip(bytes));
    const before = await sha256(bytes);
    const isOriginal = before === manifest.original.sha256;
    if (!check("TASCAM V2.03 build 217", isOriginal,
        isOriginal ? `${name}: SHA-256 matches.`
          : `${name} is not TASCAM's ${manifest.original.name}: its SHA-256 is ${before}. Download V2.03 again from TASCAM.`)) return;
    const patched = applyPatch(bytes);
    const after = await sha256(patched);
    const isExpected = after === manifest.patched.sha256;
    if (!check(`Patched file B${manifest.build}`, isExpected,
        isExpected ? `${manifest.patched.name}: SHA-256 matches the published build.`
          : `The result's SHA-256 is ${after}, not the published one. Don't install it; please report this.`)) return;
    link.href = URL.createObjectURL(new Blob([patched], { type: "application/octet-stream" }));
    link.download = manifest.patched.name;
    link.textContent = `Save ${manifest.patched.name}`;
    link.hidden = false;
  } catch (error) {
    check("Reading the file", false, error.message);
  }
}

$("file").addEventListener("change", (e) => e.target.files[0] && handle(e.target.files[0]));
const drop = $("drop");
drop.addEventListener("dragover", (e) => { e.preventDefault(); drop.classList.add("over"); });
drop.addEventListener("dragleave", () => drop.classList.remove("over"));
drop.addEventListener("drop", (e) => {
  e.preventDefault();
  drop.classList.remove("over");
  if (e.dataTransfer.files[0]) handle(e.dataTransfer.files[0]);
});
loadPatch().catch((error) => { $("build").textContent = `Could not load the patch: ${error.message}`; });
