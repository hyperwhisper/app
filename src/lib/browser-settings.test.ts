import { test, expect } from "bun:test";
import { BROWSER_SETTING_KEYS } from "./browser-settings";

// Rust only copies over the names it is sent. A name dropped from this list is
// a setting that silently resets to its default on the next launch, so pin the
// list rather than trusting a reader to notice.
test("every setting the browser used to hold is handed over", () => {
  expect([...BROWSER_SETTING_KEYS].sort()).toEqual(["active_local_model_id"]);
});

test("no name is listed twice", () => {
  expect(new Set(BROWSER_SETTING_KEYS).size).toBe(BROWSER_SETTING_KEYS.length);
});
