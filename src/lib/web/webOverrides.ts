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
