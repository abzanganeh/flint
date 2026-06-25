import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// jsdom omits Element.scrollIntoView, which several panels call during render
// to keep the bottom of a list visible. Stub it so component tests don't crash.
if (typeof window !== "undefined" && !Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

afterEach(() => {
  cleanup();
});
