/**
 * Web-build CSS overrides.
 *
 * The web modal host renders at z-index 1000, but Radix portals its popper
 * content (Select/DropdownMenu/Popover menus, dialogs) to document.body with
 * the app's default z-50 — which would land BEHIND the modal. Raise portaled
 * layers above the modal host in web builds only.
 */
if (typeof document !== "undefined" && !document.getElementById("nyaterm-web-overrides")) {
  const style = document.createElement("style");
  style.id = "nyaterm-web-overrides";
  style.textContent = `
[data-radix-popper-content-wrapper] { z-index: 1500 !important; }
body > div:has([role="listbox"]) { z-index: 1500 !important; }
body > div:has([role="menu"]) { z-index: 1500 !important; }
body > div:has([data-radix-select-viewport]) { z-index: 1500 !important; }
[role="dialog"][data-state="open"], [role="alertdialog"][data-state="open"] { z-index: 1500 !important; }
`;
  document.head.append(style);
}

/**
 * Radix Select sizes its popper content against the browser viewport
 * (window.innerHeight), so inside a dialog the dropdown can extend past the
 * dialog box. Clamp portaled popper panels to the open modal box: the panel
 * keeps its Radix-computed position, but its max-height shrinks so it stays
 * visually contained (the inner viewport scrolls the rest).
 */
if (typeof MutationObserver !== "undefined") {
  let raf = 0;
  const clampPopperPanels = () => {
    const host = document.querySelector("[data-nyaterm-web-modal-host]");
    const box = host?.querySelector(".fixed.inset-0 > div")?.getBoundingClientRect();
    if (!box) return;
    for (const portal of document.body.children) {
      if (!(portal instanceof HTMLElement) || portal === host) continue;
      // Clamp the SELECT VIEWPORT height (not the Radix-positioned panel —
      // Radix does not re-position when the viewport resizes).
      const viewport = portal.querySelector("[data-radix-select-viewport]");
      if (!(viewport instanceof HTMLElement)) continue;
      const panel = viewport.closest("div[style]") ?? portal;
      const panelRect = panel.getBoundingClientRect();
      if (panelRect.height === 0) continue;
      const listbox = viewport.querySelector("[role='listbox']");
      const selected = viewport.querySelector("[data-highlighted], [aria-selected='true']");
      const anchor = selected ?? listbox?.firstElementChild ?? viewport;
      const anchorRect = anchor.getBoundingClientRect();
      const maxH = Math.max(140, Math.round(box.bottom - anchorRect.top - 16));
      if (Math.round(viewport.getBoundingClientRect().height) > maxH) {
        viewport.style.maxHeight = `${maxH}px`;
      }
    }
  };
  const observer = new MutationObserver(() => {
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(clampPopperPanels);
  });
  observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["style", "data-state"] });
}
