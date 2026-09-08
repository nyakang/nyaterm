/**
 * Optional collision boundary for Radix popper content (Select dropdowns).
 *
 * The web modal host sets it to the open modal box so dropdowns flip and
 * shrink to stay contained within the dialog (matching the desktop window
 * behavior) instead of overflowing past the dialog edges. Desktop never sets
 * it, so Radix keeps its default viewport-based collision handling.
 */
let boundary: HTMLElement | null = null;

export function setPopoverBoundary(el: HTMLElement | null): void {
  boundary = el;
}

export function getPopoverBoundary(): HTMLElement | null {
  return boundary;
}
