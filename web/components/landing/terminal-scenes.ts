/**
 * The three flows the hero cycles through, written to match
 * crates/gateway-cli/src/presentation.rs — same glyphs, same row order, same
 * colours the CLI paints with ANSI.
 *
 * IDs and addresses are elided so no line exceeds 44 characters; the panel
 * clips rather than scrolls, and the hero column is narrow on a phone.
 */

export type Tone = "dim" | "text" | "id" | "ok" | "warn";
export type Span = readonly [text: string, tone?: Tone | undefined];
export type Line = readonly Span[];

export interface Scene {
  /** Names this scene in the window chrome; all three are always listed. */
  label: string;
  /** Typed out character by character before the body appears. */
  command: readonly Line[];
  /** Body states, shown in order. One frame for a reveal, several for a redraw. */
  frames: readonly (readonly Line[])[];
  /** Dwell between frames, and on the final frame before the next scene. */
  frameMs: number;
  holdMs: number;
}

export const MAX_WIDTH = 38;

const ID = "pay_01a0…eb5";
const BLANK: Line = [["", "dim"]];

/** Turns a block into frames that reveal one line at a time. */
function reveal(lines: readonly Line[]): readonly (readonly Line[])[] {
  return lines.map((_, index) => lines.slice(0, index + 1));
}

const rule = (title: string) => {
  const head = `── ${title} `;
  return [[head, "dim"], ["─".repeat(Math.max(0, MAX_WIDTH - head.length)), "dim"]] as const;
};

const created: readonly Line[] = [
  BLANK,
  [["Payment created · ", "text"], [ID, "id"]],
  BLANK,
  [["  Share     ", "dim"], [`payday.sh/pay/${ID}`, "id"]],
  BLANK,
  [["  ◐  awaiting payment", "warn"]],
  [["  Awaiting 25.00 USDC", "text"]],
  BLANK,
  rule("Payment details"),
  [["  Pay to    ", "dim"], ["0xfF70…A0dF", "id"], ["  native USDC", "dim"]],
  [["  Expires   ", "dim"], ["in 1 hour", "text"]],
  [["  Payout    ", "dim"], ["0x7099…79C8", "id"]],
  [["  Refund    ", "dim"], ["0x3C44…93BC", "id"]],
];

const listed: readonly Line[] = [
  BLANK,
  [["  PAYMENT         STATUS       AMOUNT", "dim"]],
  [["  ", "dim"], ["─".repeat(MAX_WIDTH - 2), "dim"]],
  [["  pay_01a0…eb5   ", "text"], ["◐ awaiting", "warn"], ["   25.00", "text"]],
  [["  pay_01a0…252   ", "text"], ["◑ partial ", "warn"], ["   40.00", "text"]],
  [["  pay_01a0…711   ", "text"], ["✓ paid    ", "ok"], ["   12.50", "text"]],
  [["  pay_01a0…c4d   ", "text"], ["✓ paid    ", "ok"], ["  120.00", "text"]],
  [["  pay_01a0…f80   ", "text"], ["✗ expired ", "dim"], ["    8.00", "text"]],
];

const transfersRule = rule("Transfers");

/** Each frame redraws in place, the way `--watch` repaints a terminal. */
const watched: readonly (readonly Line[])[] = [
  [
    BLANK,
    [["  ◐  awaiting payment", "warn"]],
    [["  Awaiting 25.00 USDC", "text"]],
    BLANK,
    transfersRule,
    [["  waiting for finalized transfers…", "dim"]],
  ],
  [
    BLANK,
    [["  ◑  partially paid", "warn"]],
    [["  10.00 of 25.00 · 15.00 left", "text"]],
    BLANK,
    transfersRule,
    [["  ↑  ", "ok"], ["+10.00 USDC", "ok"], ["  from 0xA1b2…9f3c", "dim"]],
  ],
  [
    BLANK,
    [["  ◉  payment confirmed", "ok"]],
    [["  25.00 of 25.00 USDC received", "text"]],
    BLANK,
    transfersRule,
    [["  ↑  ", "ok"], ["+10.00 USDC", "ok"], ["  from 0xA1b2…9f3c", "dim"]],
    [["  ↑  ", "ok"], ["+15.00 USDC", "ok"], ["  from 0xA1b2…9f3c", "dim"]],
  ],
  [
    BLANK,
    [["  ✓  paid", "ok"]],
    [["  25.00 of 25.00 USDC received", "text"]],
    BLANK,
    transfersRule,
    [["  ↑  ", "ok"], ["+10.00 USDC", "ok"], ["  from 0xA1b2…9f3c", "dim"]],
    [["  ↑  ", "ok"], ["+15.00 USDC", "ok"], ["  from 0xA1b2…9f3c", "dim"]],
    BLANK,
    [["  Settlement  ", "dim"], ["0x0021…aa0190", "id"]],
    [["  Settled at  ", "dim"], ["14:02:11 UTC", "text"]],
  ],
];

export const SCENES: readonly Scene[] = [
  {
    label: "create",
    command: [
      [["$ ", "dim"], ["payday create --amount 25.00 \\", "text"]],
      [["    --to 0x7099…79C8 --expires-in 1h", "text"]],
    ],
    frames: reveal(created),
    frameMs: 90,
    holdMs: 2600,
  },
  {
    label: "list",
    command: [[["$ ", "dim"], ["payday list --limit 3", "text"]]],
    frames: reveal(listed),
    frameMs: 110,
    holdMs: 2400,
  },
  {
    label: "get --watch",
    command: [[["$ ", "dim"], [`payday get ${ID} --watch`, "text"]]],
    frames: watched,
    frameMs: 1500,
    holdMs: 2600,
  },
];

/** Tallest scene, so the panel reserves its height and never jumps. */
export const MAX_LINES = Math.max(
  ...SCENES.map((scene) => scene.command.length + Math.max(...scene.frames.map((f) => f.length))),
);
