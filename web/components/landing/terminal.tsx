"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { cn } from "@/lib/cn";
import { MAX_LINES, SCENES, type Line, type Tone } from "./terminal-scenes";

const tones: Record<Tone, string> = {
  dim: "text-term-dim",
  text: "text-term-text",
  id: "text-term-id",
  ok: "text-term-accent",
  warn: "text-term-warn",
};

/** The window controls every terminal has. */
const CONTROLS = ["#ff5f57", "#febc2e", "#28c840"] as const;

/** Line box height, as a multiple of the font size. Must match `leading-`. */
const LINE_HEIGHT = 1.75;

interface Position {
  scene: number;
  /** Characters of the command typed so far. */
  typed: number;
  /** Index into the scene's body frames. */
  frame: number;
}

function commandLength(scene: number): number {
  const lines = SCENES[scene]?.command ?? [];
  return lines.reduce((total, line) => total + line.reduce((n, [text]) => n + text.length, 0), 0);
}

/** The first scene, fully drawn — what the server renders and a reader sees first. */
function settled(): Position {
  const scene = 0;
  return { scene, typed: commandLength(scene), frame: (SCENES[scene]?.frames.length ?? 1) - 1 };
}

function next(position: Position): { at: Position; delay: number } {
  const scene = SCENES[position.scene];
  if (!scene) return { at: settled(), delay: 1000 };

  const typedTotal = commandLength(position.scene);
  if (position.typed < typedTotal) {
    return { at: { ...position, typed: position.typed + 1 }, delay: 24 };
  }
  if (position.frame < scene.frames.length - 1) {
    return {
      at: { ...position, frame: position.frame + 1 },
      delay: position.frame < 0 ? 320 : scene.frameMs,
    };
  }
  return {
    at: { scene: (position.scene + 1) % SCENES.length, typed: 0, frame: -1 },
    delay: scene.holdMs,
  };
}

/** Truncates a line to the first `budget` characters, for the typing effect. */
function clip(line: Line, budget: number): { spans: Line; used: number } {
  const spans: Array<readonly [string, Tone | undefined]> = [];
  let used = 0;
  for (const [text, tone] of line) {
    if (used >= budget) break;
    const take = text.slice(0, budget - used);
    spans.push([take, tone]);
    used += take.length;
  }
  return { spans, used };
}

/**
 * One terminal line. The caret is rendered inside the line it belongs to: as a
 * sibling of these blocks it would sit in an anonymous block of its own, both
 * dropping to the next line and adding a row of height mid-scene.
 */
function Row({ line, caret = false }: { line: Line; caret?: boolean }) {
  const empty = line.every(([text]) => text === "");
  return (
    <div>
      {line.map(([text, tone], index) => (
        <span key={index} className={tones[tone ?? "text"]}>
          {text}
        </span>
      ))}
      {caret ? (
        <span className="ml-px inline-block h-[1em] w-[0.55ch] translate-y-[0.15em] animate-pulse bg-term-text" />
      ) : null}
      {/* A blank row still has to occupy a line. */}
      {empty && !caret ? " " : null}
    </div>
  );
}

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

function subscribeToMotionPreference(onChange: () => void): () => void {
  const query = window.matchMedia(REDUCED_MOTION);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

function usePrefersReducedMotion(): boolean {
  return useSyncExternalStore(
    subscribeToMotionPreference,
    () => window.matchMedia(REDUCED_MOTION).matches,
    // On the server there is no preference to read; the first frame is static
    // either way, so this only decides whether the loop starts after hydration.
    () => false,
  );
}

/**
 * The hero terminal: `create`, `list`, and `get --watch` in a loop.
 *
 * It is server-rendered fully drawn on the first scene, so the panel is never
 * blank and the page reads correctly without JavaScript; the animation picks up
 * from there on mount. `prefers-reduced-motion` leaves it on that frame.
 *
 * The body is a fixed number of line boxes tall — the tallest scene, asserted
 * by terminal-scenes.test.ts. Sizing to content would make the window grow and
 * shrink as scenes change, which shifts everything below it on the page.
 */
export function TerminalDemo({ className }: { className?: string }) {
  const [position, setPosition] = useState<Position>(settled);
  const animate = !usePrefersReducedMotion();

  useEffect(() => {
    if (!animate) return;
    const step = next(position);
    const timer = setTimeout(() => setPosition(step.at), step.delay);
    return () => clearTimeout(timer);
  }, [animate, position]);

  const scene = SCENES[position.scene] ?? SCENES[0]!;

  const command: Line[] = [];
  let budget = position.typed;
  for (const line of scene.command) {
    const { spans, used } = clip(line, budget);
    command.push(spans);
    budget -= used;
  }
  const typing = position.typed < commandLength(position.scene);
  const body = position.frame >= 0 ? (scene.frames[position.frame] ?? []) : [];

  return (
    <div
      className={cn(
        "overflow-hidden rounded-[16px] border border-term-line bg-term-bg shadow-sm",
        className,
      )}
    >
      <div className="flex items-center gap-3 border-b border-term-line bg-term-chrome px-4 py-2.5">
        <span className="flex shrink-0 gap-1.5" aria-hidden>
          {CONTROLS.map((color) => (
            <span key={color} className="size-2.5 rounded-full" style={{ backgroundColor: color }} />
          ))}
        </span>
        <span
          className="ml-auto flex min-w-0 items-center gap-3 overflow-hidden font-mono text-[11px] tabular"
          aria-hidden
        >
          {SCENES.map((entry, index) => (
            <span
              key={entry.label}
              data-scene={entry.label}
              data-active={index === position.scene}
              className={cn(
                "transition-colors duration-300",
                index === position.scene ? "text-term-text" : "text-term-dim/55",
              )}
            >
              {entry.label}
            </span>
          ))}
        </span>
      </div>

      <div
        className="overflow-hidden px-4 py-4 font-mono leading-[1.75] whitespace-pre sm:px-5"
        // The panel clips rather than scrolls, so the type scales with the
        // viewport to keep the widest line (MAX_WIDTH) inside it down to 320px.
        style={{ fontSize: "clamp(10px, 2.6vw, 12.5px)" }}
        // The loop is decoration; a screen reader gets the still description below.
        aria-hidden
      >
        {/* Height lives here, inside the padding, so it is exactly N line boxes. */}
        <div style={{ height: `${MAX_LINES * LINE_HEIGHT}em` }}>
          {command.map((line, index) => (
            <Row key={`c${index}`} line={line} caret={typing && index === command.length - 1} />
          ))}
          {body.map((line, index) => (
            <Row key={`b${position.frame}-${index}`} line={line} />
          ))}
        </div>
      </div>

      <p className="sr-only">
        A terminal running the Payday CLI: creating a 25.00 USDC payment and printing its shareable
        link, listing recent payments, then watching one settle from awaiting to paid.
      </p>
    </div>
  );
}
