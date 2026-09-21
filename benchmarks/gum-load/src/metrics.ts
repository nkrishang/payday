/**
 * Distribution math and terminal-friendly renderings. Pure functions so the
 * reducer, live view, and offline report share one implementation.
 */

export function percentile(sortedAscending: number[], p: number): number | undefined {
  if (sortedAscending.length === 0) return undefined;
  const rank = Math.ceil((p / 100) * sortedAscending.length);
  return sortedAscending[Math.min(sortedAscending.length, Math.max(1, rank)) - 1]!;
}

export interface Distribution {
  count: number;
  p50?: number;
  p95?: number;
  p99?: number;
  max?: number;
  mean?: number;
}

export function distribution(samples: number[]): Distribution {
  if (samples.length === 0) return { count: 0 };
  const sorted = [...samples].sort((a, b) => a - b);
  const mean = sorted.reduce((sum, value) => sum + value, 0) / sorted.length;
  return {
    count: sorted.length,
    p50: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    p99: percentile(sorted, 99),
    max: sorted[sorted.length - 1],
    mean,
  };
}

export function fmtSeconds(seconds: number | undefined, digits = 2): string {
  if (seconds === undefined) return "—";
  if (seconds >= 100) return `${seconds.toFixed(0)}s`;
  if (seconds >= 10) return `${seconds.toFixed(1)}s`;
  return `${seconds.toFixed(digits)}s`;
}

export function fmtUnits(units: string | number, decimals = 6): string {
  const value = typeof units === "number" ? BigInt(Math.round(units)) : BigInt(units);
  const negative = value < 0n;
  const abs = negative ? -value : value;
  const text = abs.toString().padStart(decimals + 1, "0");
  const whole = text.slice(0, text.length - decimals);
  const frac = text.slice(text.length - decimals).replace(/0+$/, "");
  return `${negative ? "-" : ""}${whole}${frac ? `.${frac}` : ""}`;
}

const SPARK_GLYPHS = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

export function sparkline(samples: number[], width: number): string {
  if (samples.length === 0) return " ".repeat(width);
  const recent = samples.slice(-width);
  const max = Math.max(...recent);
  const min = Math.min(...recent);
  const span = max - min || 1;
  return recent
    .map((value) => SPARK_GLYPHS[Math.min(SPARK_GLYPHS.length - 1, Math.floor(((value - min) / span) * SPARK_GLYPHS.length))])
    .join("")
    .padStart(width, " ");
}

/** Horizontal histogram with fixed bin count over the observed range. */
export function histogram(samples: number[], bins: number, width: number): Array<{ label: string; bar: string; count: number }> {
  if (samples.length === 0) return [];
  const min = Math.min(...samples);
  const max = Math.max(...samples);
  const span = max - min || 1;
  const counts = new Array<number>(bins).fill(0);
  for (const value of samples) {
    const bucket = Math.min(bins - 1, Math.floor(((value - min) / span) * bins));
    counts[bucket] += 1;
  }
  const peak = Math.max(...counts, 1);
  return counts.map((count, i) => {
    const low = min + (span * i) / bins;
    const high = min + (span * (i + 1)) / bins;
    return {
      label: `${low.toFixed(1)}–${high.toFixed(1)}s`,
      bar: "█".repeat(Math.max(count > 0 ? 1 : 0, Math.round((count / peak) * width))),
      count,
    };
  });
}

export function rate(count: number, seconds: number): string {
  if (seconds <= 0) return "—";
  return `${(count / seconds).toFixed(2)}/s`;
}
