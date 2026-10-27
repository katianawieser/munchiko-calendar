// Color rows on the fabric and accessory forms (templates/color_fields.html).
//
// Progressive enhancement: the form works without this script (names only).
// With it, each row gets a swatch picker, colors can be added/removed, and
// colors from the item's photos or from the rest of the stash ("Your shades")
// can be added with one click. Picking a shade for a row with no name asks
// the server for the nearest name (GET /colors/name), filled in as a
// suggestion that typing replaces.
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

  // Fill an empty name (or replace an earlier suggestion) with the nearest
  // name for the shade. A name the user typed is never touched.
  async function suggestName(row, hex) {
    const name = row.querySelector('input[name="color_name"]');
    if (name.value.trim() !== "" && name.dataset.suggested !== "yes") return;
    try {
      const reply = await fetch(`/colors/name?hex=${encodeURIComponent(hex)}`);
      if (!reply.ok) return;
      name.value = await reply.text();
      name.dataset.suggested = "yes";
      name.classList.add("suggested");
      confirmShade(row, "✓ shade set · name suggested, change it if you like");
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

  // Color chips: "From your photos" and "Your shades". Which row a chip
  // fills, in order of preference:
  //   1. the row whose name box was clicked into last: it gets the exact
  //      shade and keeps its own name ("faded orange" stays "faded orange");
  //   2. a row that already has the chip's name (it gains the exact shade);
  //   3. an empty row, then 4. a new row; these take the chip's name too.
  const chipBoxes = list.querySelectorAll(".color-ideas, .your-shades");
  if (chipBoxes.length > 0) {
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

    list.querySelectorAll(".color-idea").forEach((button) => {
      button.addEventListener("click", () => {
        const { name, hex } = button.dataset;
        const all = [...rows.querySelectorAll(".color-row")];
        let row = chosenRow && rows.contains(chosenRow) ? chosenRow : null;
        chosenRow = null; // one chip per click into a row
        row =
          row ||
          all.find((r) => nameOf(r) === name) ||
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
