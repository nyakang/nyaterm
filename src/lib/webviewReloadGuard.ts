const TERMINAL_ROOT_SELECTOR = '[data-terminal-root="true"]';
const REMOTE_DESKTOP_INPUT_ROOT_SELECTOR =
  '[data-remote-desktop-input-root="true"], [data-rdp-input-root="true"]';

let installed = false;

function isElement(value: EventTarget | null): value is Element {
  return value instanceof Element;
}

function eventPathContainsRoot(event: KeyboardEvent, selector: string) {
  return event.composedPath().some((target) => {
    if (!isElement(target)) return false;
    return target.matches(selector);
  });
}

function eventTargetIsInsideRoot(event: KeyboardEvent, selector: string) {
  if (eventPathContainsRoot(event, selector)) return true;
  const target = event.target;
  return isElement(target) && target.closest(selector) !== null;
}

function isReloadShortcut(event: KeyboardEvent) {
  if (event.altKey) return false;
  if (event.code === "F5") return true;
  return (event.ctrlKey || event.metaKey) && event.code === "KeyR";
}

function isHardReloadShortcut(event: KeyboardEvent) {
  return (
    (event.ctrlKey || event.metaKey) &&
    event.shiftKey &&
    !event.altKey &&
    event.code === "KeyR"
  );
}

function isPrintShortcut(event: KeyboardEvent) {
  if (event.code !== "KeyP" || event.altKey || event.shiftKey) return false;

  const isMac = navigator.userAgent.includes("Mac");
  if (isMac) return event.metaKey && !event.ctrlKey;
  return event.ctrlKey && !event.metaKey;
}

function isReservedWebviewShortcut(event: KeyboardEvent) {
  return isReloadShortcut(event) || isPrintShortcut(event);
}

function preventReservedWebviewShortcut(event: KeyboardEvent) {
  const insideTerminal = eventTargetIsInsideRoot(event, TERMINAL_ROOT_SELECTOR);

  if (insideTerminal) {
    if (isHardReloadShortcut(event)) {
      event.preventDefault();
    }
    return;
  }

  if (isReservedWebviewShortcut(event)) {
    event.preventDefault();
    if (!eventTargetIsInsideRoot(event, REMOTE_DESKTOP_INPUT_ROOT_SELECTOR)) {
      event.stopPropagation();
    }
  }
}

export function installWebviewReloadGuard() {
  if (installed) return;
  installed = true;
  window.addEventListener("keydown", preventReservedWebviewShortcut, true);
}
