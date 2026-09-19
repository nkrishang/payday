/**
 * Minimal double-buffered ANSI renderer: diff the row arrays, repaint only
 * what changed, cap the frame rate, and always restore the terminal. The
 * renderer knows nothing about payments; it paints rows handed to it.
 */

import readline from "node:readline";
import type { RunState } from "../model.js";
import type { Journal } from "../journal.js";

export interface Display {
  kind: "tui" | "plain" | "json";
  update(): void;
  stop(): void;
}

export interface RenderLoopOptions {
  onQuit: () => void;
  input?: NodeJS.ReadStream;
  journal?: Journal;
}

const MAX_FPS = 10;

export function renderLoop(
  state: RunState,
  buildRows: () => string[],
  options: RenderLoopOptions,
): Display {
  const out = process.stdout;
  const input = options.input ?? process.stdin;
  let stopped = false;
  let lastRows: string[] = [];
  let lastPaint = 0;
  let pendingUpdate = false;

  const enter = (): void => {
    out.write("\x1b[?1049h"); // alternate screen
    out.write("\x1b[?25l"); // hide cursor
    out.write("\x1b[2J\x1b[H");
    if (input.isTTY) {
      readline.emitKeypressEvents(input);
      input.setRawMode(true);
      input.resume();
      input.on("keypress", onKeypress);
    }
    process.on("SIGINT", onSigint);
    out.on("resize", () => (lastRows = []));
  };

  const exit = (): void => {
    out.write("\x1b[0m");
    out.write("\x1b[?25h"); // cursor back
    out.write("\x1b[?1049l"); // main screen
    if (input.isTTY && input.setRawMode) {
      input.setRawMode(false);
      input.removeListener("keypress", onKeypress);
      input.pause();
    }
    process.removeListener("SIGINT", onSigint);
  };

  const onKeypress = (_: string, key: { name?: string; ctrl?: boolean }): void => {
    if (key?.name === "q" || (key?.ctrl && key?.name === "c")) {
      options.onQuit();
    } else if (key?.name === "p") {
      state.guard.paused = !state.guard.paused;
    }
  };

  const onSigint = (): void => {
    options.onQuit();
  };

  const paint = (): void => {
    if (stopped) return;
    const now = performance.now();
    if (now - lastPaint < 1000 / MAX_FPS) {
      pendingUpdate = true;
      return;
    }
    lastPaint = now;
    pendingUpdate = false;

    const width = out.columns || 120;
    const rows = buildRows().map((row) => stripToWidth(row, width));
    // Diff against the last frame; repaint the tail from the first change.
    let firstChanged = rows.findIndex((row, i) => lastRows[i] !== row);
    if (firstChanged === -1 && rows.length === lastRows.length) return;
    if (firstChanged === -1) firstChanged = Math.min(lastRows.length, rows.length);
    const lines: string[] = [];
    if (rows.length < lastRows.length) lines.push(`\x1b[${rows.length + 1};1H\x1b[0J`); // clear below
    lines.push(`\x1b[${firstChanged + 1};1H`);
    for (let i = firstChanged; i < rows.length; i++) {
      lines.push(rows[i]!);
      if (i < rows.length - 1) lines.push("\n");
    }
    out.write(lines.join(""));
    lastRows = rows;
  };

  const interval = setInterval(() => paint(), 100);
  enter();
  paint();

  return {
    kind: "tui",
    update: () => paint(),
    stop: () => {
      if (stopped) return;
      stopped = true;
      clearInterval(interval);
      exit();
      options.journal?.flush();
    },
  };
}

/** Runs a background updater for non-TUI displays. */
export function plainLoop(display: Display, periodMs = 2000): NodeJS.Timeout {
  return setInterval(() => display.update(), periodMs);
}

function stripToWidth(row: string, width: number): string {
  const visible = row.replace(/\x1b\[[0-9;]*m/g, "");
  if (visible.length <= width) return row + " ".repeat(width - visible.length);
  // Truncate by visible length; ANSI codes around the cut are dropped.
  return visible.slice(0, width - 1) + "…";
}

export function installInput(stream: NodeJS.ReadStream = process.stdin): NodeJS.ReadStream {
  return stream;
}
