// Photo boxes (templates/photo_input.html and photos_section.html).
//
// Progressive enhancement: without this script, photos still upload when the
// form is submitted. With it:
// - photos chosen in a fabric/accessory form show straight away, before
//   saving, so the color picker's eyedropper can sample them;
// - on a post page, choosing photos uploads them at once (no extra button).
"use strict";

// Browsers can show these directly; HEIC and TIFF get a label instead.
const SHOWABLE = ["image/jpeg", "image/png", "image/webp", "image/gif", "image/bmp"];

document.querySelectorAll("input.photo-input").forEach((input) => {
  const preview = input.closest("form").querySelector(".photo-preview");
  let urls = [];

  input.addEventListener("change", () => {
    // Free the previous previews' memory before making new ones.
    urls.forEach((url) => URL.revokeObjectURL(url));
    urls = [];
    preview.replaceChildren();

    for (const file of input.files) {
      const figure = document.createElement("figure");
      if (SHOWABLE.includes(file.type)) {
        const url = URL.createObjectURL(file);
        urls.push(url);
        const img = document.createElement("img");
        img.src = url;
        img.alt = file.name;
        figure.append(img);
      } else {
        const label = document.createElement("div");
        label.className = "no-preview";
        label.textContent = file.name;
        figure.append(label);
      }
      const caption = document.createElement("figcaption");
      caption.textContent = "not saved yet";
      figure.append(caption);
      preview.append(figure);
    }
    preview.hidden = input.files.length === 0;
  });
});

document.querySelectorAll("form.auto-upload").forEach((form) => {
  const input = form.querySelector('input[type="file"]');
  form.querySelector('button[type="submit"]').hidden = true;
  input.addEventListener("change", () => {
    if (input.files.length > 0) {
      form.submit();
    }
  });
});
