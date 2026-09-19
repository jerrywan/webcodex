import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// jsdom has no native modal implementation. Preserve semantic visibility in tests.
Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, writable: true, value() { this.setAttribute("open", ""); } });
Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, writable: true, value() { this.removeAttribute("open"); } });

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});
