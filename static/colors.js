// Color rows on the fabric and accessory forms (templates/color_fields.html).
//
// Progressive enhancement: the form works without this script (names only).
// With it, each row gets a swatch picker, colors can be added/removed, and
// colors from the item's photos or from the rest of the stash ("Your shades")
// can be added with one click. Picking a shade asks the server for the
// nearest names (GET /colors/name?count=3). An empty name (or one the page
// suggested itself) gets the best one; a name the user typed is never
// changed, and the names are offered as buttons under the row instead.
// Chips never replace a shade that's already set (see below).
//
// A color input always holds *some* color (white by default), so it can't
// say "no swatch". The picked value is therefore copied into a hidden
// color_hex field, which stays empty until the user really picks a shade.
"use strict";

document.querySelectorAll(".color-list").forEach((list) => {
  const rows = list.querySelector(".color-rows");
  const template = list.querySelector("template");
  const addButton = list.querySelector(".add-color");

  // A short "✓ shade set" next to the row, so it's clear the pick took.
  function confirmShade(row, text) {
    const status = row.querySelector(".shade-status");
    status.textContent = text;
    clearTimeout(status.timer);
    status.timer = setTimeout(() => (status.textContent = ""), 4000);
  }

  // Name buttons under a row: "Suggested: peach · salmon · coral". Clicking
  // one makes it the row's name (the user chose it, so it's theirs).
  function offerNames(row, names) {
    let box = row.querySelector(".name-suggestions");
    if (!box) {
      box = document.createElement("div");
      box.className = "name-suggestions";
      row.append(box);
    }
    const name = row.querySelector('input[name="color_name"]');
    const current = name.value.trim().toLowerCase();
    const offered = names.filter((n) => n !== current);
    box.replaceChildren();
    box.hidden = offered.length === 0;
    if (box.hidden) return;
    const label = document.createElement("small");
    label.className = "muted";
    label.textContent = name.dataset.suggested === "yes" ? "Or: " : "Suggested: ";
    box.append(label);
    for (const suggestion of offered) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "outline secondary small name-choice";
      button.textContent = suggestion;
      button.addEventListener("click", () => {
        name.value = suggestion;
        delete name.dataset.suggested;
        name.classList.remove("suggested");
        box.hidden = true;
      });
      box.append(button);
    }
  }

  // Ask for the nearest names for the shade. An empty name (or the page's
  // own earlier suggestion) takes the best one; the others, or all of them
  // when the user typed the name, are offered as buttons.
  async function suggestName(row, hex) {
    const name = row.querySelector('input[name="color_name"]');
    try {
      const reply = await fetch(`/colors/name?count=3&hex=${encodeURIComponent(hex)}`);
      if (!reply.ok) return;
      const names = (await reply.text()).split("\n").filter(Boolean);
      if (names.length === 0) return;
      if (name.value.trim() === "" || name.dataset.suggested === "yes") {
        name.value = names[0];
        name.dataset.suggested = "yes";
        name.classList.add("suggested");
        confirmShade(row, "✓ shade set · name suggested, change it if you like");
      }
      offerNames(row, names);
    } catch {
      // No name suggestion is fine; the shade is still set.
    }
  }

  function wire(row) {
    const picker = row.querySelector(".swatch-picker");
    const hex = row.querySelector('input[name="color_hex"]');
    const name = row.querySelector('input[name="color_name"]');
    const clear = row.querySelector(".clear-swatch");
    const remove = row.querySelector(".remove-color");

    // Faded picker + no "No swatch" button = nothing picked yet.
    const show = () => {
      picker.classList.toggle("unset", hex.value === "");
      clear.hidden = hex.value === "";
    };
    const pick = () => {
      hex.value = picker.value;
      show();
    };
    // Once a shade is settled on: confirm it and suggest a name. "change"
    // means the picker was closed on a shade, but on a Mac, Firefox (and
    // Safari) use the system color panel, which can stay open while you carry
    // on, so "change" may never come. A shade that stops changing for a
    // moment counts as settled too.
    let settleTimer;
    const settled = () => {
      clearTimeout(settleTimer);
      confirmShade(row, "✓ shade set");
      suggestName(row, picker.value);
    };
    picker.addEventListener("input", () => {
      pick();
      clearTimeout(settleTimer);
      settleTimer = setTimeout(settled, 750);
    });
    picker.addEventListener("change", () => {
      pick();
      settled();
    });
    // Typing makes the name the user's own.
    name.addEventListener("input", () => {
      delete name.dataset.suggested;
      name.classList.remove("suggested");
    });
    clear.addEventListener("click", () => {
      hex.value = "";
      show();
      confirmShade(row, "swatch removed");
    });
    remove.addEventListener("click", () => row.remove());

    picker.hidden = false;
    remove.hidden = false;
    show();
  }

  function addRow() {
    const row = template.content.firstElementChild.cloneNode(true);
    rows.append(row);
    wire(row);
    return row;
  }

  rows.querySelectorAll(".color-row").forEach(wire);
  addButton.hidden = false;
  addButton.addEventListener("click", () => {
    addRow().querySelector('input[name="color_name"]').focus();
  });

  // Color chips: "From your photos" and "Your shades". A chip never
  // replaces a shade that's already set (that used to be easy to do by
  // accident). Which row it fills, in order of preference:
  //   1. the row whose name box was clicked into last, if it has no shade
  //      yet: it gets the shade and keeps its own name;
  //   2. a row with the chip's name and no shade yet;
  //   3. an empty row, then 4. a new row; these take the chip's name too.
  // To change a shade that's set, use the row's own picker (or "No swatch"
  // first, then a chip).
  const chipBoxes = list.querySelectorAll(".color-ideas:not(.chosen-photo-ideas), .your-shades");
  chipBoxes.forEach((box) => (box.hidden = false));

  // Remember the row being worked on. Clicking a chip moves focus to the
  // chip, so only focus moving anywhere *else* forgets the row.
  let chosenRow = null;
  document.addEventListener("focusin", (event) => {
    if (event.target.matches('.color-row input[name="color_name"]')) {
      chosenRow = rows.contains(event.target) ? event.target.closest(".color-row") : null;
    } else if (!event.target.closest(".color-ideas, .your-shades")) {
      chosenRow = null;
    }
  });

  const nameBox = (row) => row.querySelector('input[name="color_name"]');
  const nameOf = (row) => nameBox(row).value.trim().toLowerCase();
  const hexOf = (row) => row.querySelector('input[name="color_hex"]').value;

  function wireChip(button) {
    button.addEventListener("click", () => {
      const { name, hex } = button.dataset;
      const all = [...rows.querySelectorAll(".color-row")];
      const open = (r) => r && rows.contains(r) && hexOf(r) === "";
      let row = open(chosenRow) ? chosenRow : null;
      chosenRow = null; // one chip per click into a row
      row =
        row ||
        all.find((r) => nameOf(r) === name && hexOf(r) === "") ||
        all.find((r) => nameOf(r) === "" && hexOf(r) === "") ||
        addRow();
      if (nameOf(row) === "" || nameBox(row).dataset.suggested === "yes") {
        nameBox(row).value = name;
        delete nameBox(row).dataset.suggested;
        nameBox(row).classList.remove("suggested");
      }
      const picker = row.querySelector(".swatch-picker");
      picker.value = hex;
      // Let the row's own handler copy the shade into the hidden field.
      picker.dispatchEvent(new Event("input"));
      confirmShade(row, "✓ shade set");
      button.disabled = true;
      button.title = `Added ${name}`;
    });
  }
  list.querySelectorAll(".color-idea").forEach(wireChip);

  // Photos chosen in this form but not saved yet: send them (and the colors
  // already in the form) to the server, which finds their main colors with
  // the same Rust code as for saved photos and replies with ready-made chips.
  const form = list.closest("form");
  const photoInput = form && form.querySelector("input.photo-input");
  const chosenBox = list.querySelector(".chosen-photo-ideas");
  if (photoInput && chosenBox) {
    let latest = 0; // only the newest choice's answer is shown
    const say = (text) => {
      const note = document.createElement("small");
      note.className = "muted";
      note.textContent = text;
      chosenBox.replaceChildren(note);
    };
    photoInput.addEventListener("change", async () => {
      const ask = ++latest;
      if (photoInput.files.length === 0) {
        chosenBox.hidden = true;
        chosenBox.replaceChildren();
        return;
      }
      chosenBox.hidden = false;
      say("Finding the photos' colors…");
      const data = new FormData();
      for (const file of photoInput.files) data.append("photo", file);
      rows.querySelectorAll(".color-row").forEach((row) => {
        data.append("color_name", nameBox(row).value);
        data.append("color_hex", hexOf(row));
      });
      try {
        const reply = await fetch("/colors/from-photos", { method: "POST", body: data });
        if (ask !== latest) return;
        if (!reply.ok) throw new Error(String(reply.status));
        // Server-rendered (and escaped) by an Askama template.
        chosenBox.innerHTML = await reply.text();
        chosenBox.querySelectorAll(".color-idea").forEach(wireChip);
      } catch {
        if (ask === latest) say("Couldn't read the photos' colors; the photos will still be saved.");
      }
    });
  }
});

// Accessory form: show the chosen fabrics' colors under "Use colors from these
// fabrics", updating as fabrics are picked. Each fabric's chips are in a
// hidden <template>; copy the chosen ones in role order (main first) and skip
// a name that's already shown, the same rule the server uses.
document.querySelectorAll(".fabric-colors").forEach((box) => {
  const form = box.closest("form");
  const chips = box.querySelector(".fabric-color-chips");
  const none = box.querySelector(".no-fabric-colors");
  const toggle = form.querySelector('input[name="colors_from_fabrics"]');
  const selects = [...form.querySelectorAll('select[name$="_fabric"]')];

  function update() {
    box.hidden = !toggle.checked;
    chips.replaceChildren();
    const shown = new Set();
    for (const select of selects) {
      const set = form.querySelector(`template.fabric-chip-set[data-fabric="${select.value}"]`);
      if (!set) continue;
      for (const chip of set.content.children) {
        if (shown.has(chip.dataset.colorName)) continue;
        shown.add(chip.dataset.colorName);
        chips.append(chip.cloneNode(true));
      }
    }
    none.hidden = shown.size > 0;
  }

  toggle.addEventListener("change", update);
  selects.forEach((select) => select.addEventListener("change", update));
});

// Accessory form: "+ New fabric…" in a fabric select shows a name box for the
// new fabric (created when the form is saved). Without this script the boxes
// are always visible, which works too.
document.querySelectorAll("select.fabric-select").forEach((select) => {
  const box = select.closest(".fabric-role").querySelector(".new-fabric");
  const name = box.querySelector(".new-fabric-name");
  const show = (focus) => {
    const isNew = select.value === "new";
    box.hidden = !isNew;
    if (isNew && focus) name.focus();
  };
  select.addEventListener("change", () => show(true));
  show(false);
});
