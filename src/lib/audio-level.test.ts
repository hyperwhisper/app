import { test, expect } from "bun:test";
import { db } from "./audio-level";

test("loudness is shown in decibels below the loudest possible sample", () => {
  // [level, shown, why]
  const cases: [number, string, string][] = [
    [1, "0", "the loudest a sample can be"],
    [0.5, "-6", "half as loud"],
    [0.1, "-20", "quiet but usable"],
    [0.05, "-26", "normal speech close to the microphone"],
    [0.005, "-46", "the floor below which the model cannot read it"],
    [0.00002, "-94", "an almost silent microphone"],
  ];
  for (const [level, expected, why] of cases) {
    expect(db(level), why).toBe(expected);
  }
});

test("nothing at all is shown as -inf rather than a huge negative number", () => {
  // log(0) has no answer, so these are caught before the maths runs.
  const cases: [number, string][] = [
    [0, "digital silence"],
    [0.00001, "exactly on the cutoff"],
    [0.000001, "below the cutoff"],
    [-0.5, "a negative level, which should never arrive but must not crash"],
  ];
  for (const [level, why] of cases) {
    expect(db(level), why).toBe("-inf");
  }
});

test("the result always fits the space the windows reserve for it", () => {
  // Both windows pad this to 4; anything longer shifts the line sideways.
  const levels = [0, 0.00001, 0.0001, 0.001, 0.01, 0.05, 0.1, 0.5, 1];
  for (const level of levels) {
    expect(db(level).length, `level ${level}`).toBeLessThanOrEqual(4);
  }
});

// Harmless quirk, worth pinning: 0.945 to 0.999 all round to "-0".
test("levels just below the maximum round to minus zero", () => {
  expect(db(0.955)).toBe("-0");
  expect(db(1)).toBe("0");
});
