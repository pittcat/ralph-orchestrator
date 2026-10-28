/**
 * Vitest test setup
 *
 * Configures the testing environment with jest-dom matchers
 * for enhanced DOM assertions.
 */

import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// jsdom lacks matchMedia; theme + a11y code paths call it.
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}

// jsdom lacks scrollIntoView; command palette calls it for option navigation.
if (typeof Element !== "undefined" && !Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = vi.fn();
}
