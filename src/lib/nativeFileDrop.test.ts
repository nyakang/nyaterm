import { afterEach, describe, expect, it, vi } from "vitest";
import { isDropPositionInsideElement, isDropPositionTopmostWithinElement } from "./nativeFileDrop";

const targetRect = {
  left: 10,
  top: 20,
  right: 210,
  bottom: 220,
  width: 200,
  height: 200,
  x: 10,
  y: 20,
  toJSON: () => ({}),
} as DOMRect;

const originalElementFromPoint = Object.getOwnPropertyDescriptor(document, "elementFromPoint");

afterEach(() => {
  vi.restoreAllMocks();
  document.body.replaceChildren();
  if (originalElementFromPoint) {
    Object.defineProperty(document, "elementFromPoint", originalElementFromPoint);
  } else {
    Reflect.deleteProperty(document, "elementFromPoint");
  }
});

describe("external file drop position ownership", () => {
  it("rejects a position outside the target bounds", () => {
    const target = document.createElement("div");
    vi.spyOn(target, "getBoundingClientRect").mockReturnValue(targetRect);
    const elementFromPoint = vi.fn(() => target);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: elementFromPoint,
    });

    expect(isDropPositionInsideElement({ x: 5, y: 100 }, target)).toBe(false);
    expect(isDropPositionTopmostWithinElement({ x: 5, y: 100 }, target)).toBe(false);
    expect(elementFromPoint).not.toHaveBeenCalled();
  });

  it("accepts the target itself and its descendants as the topmost element", () => {
    const target = document.createElement("div");
    const child = document.createElement("span");
    target.append(child);
    document.body.append(target);
    vi.spyOn(target, "getBoundingClientRect").mockReturnValue(targetRect);

    const elementFromPoint = vi.fn<() => Element | null>();
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: elementFromPoint,
    });

    elementFromPoint.mockReturnValueOnce(target).mockReturnValueOnce(child);

    expect(isDropPositionTopmostWithinElement({ x: 100, y: 100 }, target)).toBe(true);
    expect(isDropPositionTopmostWithinElement({ x: 100, y: 100 }, target)).toBe(true);
  });

  it("rejects a covered target even when the position is inside its bounds", () => {
    const target = document.createElement("div");
    const floatingPanel = document.createElement("aside");
    document.body.append(target, floatingPanel);
    vi.spyOn(target, "getBoundingClientRect").mockReturnValue(targetRect);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => floatingPanel),
    });

    expect(isDropPositionInsideElement({ x: 100, y: 100 }, target)).toBe(true);
    expect(isDropPositionTopmostWithinElement({ x: 100, y: 100 }, target)).toBe(false);
  });

  it("fails closed when the document cannot resolve a topmost element", () => {
    const target = document.createElement("div");
    vi.spyOn(target, "getBoundingClientRect").mockReturnValue(targetRect);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => null),
    });

    expect(isDropPositionTopmostWithinElement({ x: 100, y: 100 }, target)).toBe(false);
  });
});
