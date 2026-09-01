import { describe, expect, it } from "vitest";
import { MAX_LINES, MAX_WIDTH, SCENES, type Line } from "./terminal-scenes";

const width = (line: Line) => line.reduce((total, [text]) => total + [...text].length, 0);

/**
 * The terminal clips rather than scrolls, so a line that overflows is silently
 * cut off. On a 320px viewport the panel gives about 248px of text, which at
 * the smallest step of the clamped font is roughly 38 characters.
 */
describe("terminal scenes", () => {
  it("has no line wider than the panel can show", () => {
    const offenders: string[] = [];
    for (const scene of SCENES) {
      for (const line of [...scene.command, ...scene.frames.flat()]) {
        if (width(line) > MAX_WIDTH) {
          offenders.push(`${scene.label}: ${width(line)} chars — ${line.map(([t]) => t).join("")}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("covers the three flows the hero promises", () => {
    expect(SCENES.map((scene) => scene.label)).toEqual(["create", "list", "get --watch"]);
  });

  it("reserves height for the tallest scene so the panel never jumps", () => {
    for (const scene of SCENES) {
      const tallest = scene.command.length + Math.max(...scene.frames.map((f) => f.length));
      expect(tallest).toBeLessThanOrEqual(MAX_LINES);
    }
  });

  it("keeps every scene close to the reserved height, so the window is not mostly empty", () => {
    for (const scene of SCENES) {
      const tallest = scene.command.length + Math.max(...scene.frames.map((f) => f.length));
      expect(tallest / MAX_LINES).toBeGreaterThan(0.5);
    }
  });

  it("ends the watch flow on a settled payment, with the transfers that prove it", () => {
    const watch = SCENES[2]!;
    const last = watch.frames[watch.frames.length - 1]!;
    const text = last.flatMap((line) => line.map(([t]) => t)).join(" ");
    expect(text).toMatch(/paid/);
    expect(text).toMatch(/Settlement/);
    expect(text).toMatch(/\+10\.00 USDC/);
    expect(text).toMatch(/\+15\.00 USDC/);
  });
});
