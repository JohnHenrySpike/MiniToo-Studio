// Screenshot tabs and copy buttons; the page reads fine without them.

document.querySelectorAll(".gallery").forEach((gallery) => {
  const img = gallery.querySelector("img");
  const caption = gallery.querySelector("figcaption");
  const tabs = gallery.querySelectorAll(".tabs button");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      tabs.forEach((t) => t.setAttribute("aria-selected", t === tab ? "true" : "false"));
      img.src = gallery.dataset.base + tab.dataset.shot + ".png";
      img.alt = tab.textContent;
      caption.innerHTML = tab.dataset.caption;
    });
  });
  // Warm the cache so switching tabs doesn't flash.
  window.addEventListener("load", () => {
    tabs.forEach((t) => { new Image().src = gallery.dataset.base + t.dataset.shot + ".png"; });
  });
});

document.querySelectorAll(".code").forEach((block) => {
  const button = document.createElement("button");
  const label = document.documentElement.lang === "ru" ? ["Копировать", "Скопировано"] : ["Copy", "Copied"];
  button.type = "button";
  button.textContent = label[0];
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(block.querySelector("pre").innerText.trim());
      button.textContent = label[1];
      setTimeout(() => { button.textContent = label[0]; }, 1500);
    } catch (_) {}
  });
  block.appendChild(button);
});
