import type { ReactNode } from "react";

/**
 * The documentation's diagrams, drawn as inline SVG so they take the page's
 * palette in both colour schemes and stay crisp at any size. Each is a
 * picture of a real mechanism — the flow of one deposit, the lifecycle, how
 * the address is derived — not decoration.
 *
 * Every drawing sits in a viewBox and scales to its container; below a
 * minimum width it scrolls sideways inside the figure instead of shrinking
 * its type past legibility.
 */

const GREEN = "#a3d277";
const YELLOW = "#ead26d";
const BLUE = "#7dd3fc";

type Accent = "none" | "green" | "yellow" | "blue";

const ACCENT_STROKE: Record<Accent, string> = {
  none: "var(--line-strong)",
  green: GREEN,
  yellow: YELLOW,
  blue: BLUE,
};

function Node({
  x,
  y,
  w,
  h = 56,
  title,
  subtitle,
  accent = "none",
  mono = false,
}: {
  x: number;
  y: number;
  w: number;
  h?: number;
  title: string;
  subtitle?: string;
  accent?: Accent;
  mono?: boolean;
}) {
  const cx = x + w / 2;
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx={10}
        fill="var(--raised)"
        stroke={ACCENT_STROKE[accent]}
        strokeWidth={accent === "none" ? 1 : 1.5}
      />
      <text
        x={cx}
        y={subtitle ? y + h / 2 - 5 : y + h / 2 + 5}
        textAnchor="middle"
        fontSize={13.5}
        fontWeight={500}
        fill="var(--ink)"
        fontFamily={mono ? "var(--font-mono)" : "inherit"}
      >
        {title}
      </text>
      {subtitle ? (
        <text
          x={cx}
          y={y + h / 2 + 13}
          textAnchor="middle"
          fontSize={11.5}
          fill="var(--muted)"
          fontFamily="var(--font-mono)"
        >
          {subtitle}
        </text>
      ) : null}
    </g>
  );
}

/** An arrow through a list of points, with its head drawn on the last segment. */
function Arrow({
  points,
  label,
  labelAt = 0.5,
  labelDy = -8,
  color = "var(--line-strong)",
  dashed = false,
}: {
  points: Array<[number, number]>;
  label?: string;
  /** Where along the first segment the label sits, 0–1. */
  labelAt?: number;
  labelDy?: number;
  color?: string;
  dashed?: boolean;
}) {
  const d = points.map(([x, y], index) => `${index === 0 ? "M" : "L"}${x} ${y}`).join(" ");
  const [x1, y1] = points[points.length - 2] ?? [0, 0];
  const [x2, y2] = points[points.length - 1] ?? [0, 0];
  const angle = Math.atan2(y2 - y1, x2 - x1);
  const size = 7;
  const head = [
    [x2, y2],
    [x2 - size * Math.cos(angle - Math.PI / 6), y2 - size * Math.sin(angle - Math.PI / 6)],
    [x2 - size * Math.cos(angle + Math.PI / 6), y2 - size * Math.sin(angle + Math.PI / 6)],
  ]
    .map(([x, y]) => `${x},${y}`)
    .join(" ");
  const [lx1, ly1] = points[0] ?? [0, 0];
  const [lx2, ly2] = points[1] ?? [0, 0];
  const lx = lx1 + (lx2 - lx1) * labelAt;
  const ly = ly1 + (ly2 - ly1) * labelAt;
  return (
    <g>
      <path
        d={d}
        fill="none"
        stroke={color}
        strokeWidth={1.25}
        strokeDasharray={dashed ? "4 4" : undefined}
      />
      <polygon points={head} fill={color} />
      {label ? (
        <text
          x={lx}
          y={ly + labelDy}
          textAnchor="middle"
          fontSize={11.5}
          fill="var(--muted)"
          fontFamily="var(--font-mono)"
        >
          {label}
        </text>
      ) : null}
    </g>
  );
}

function Caption({
  x,
  y,
  children,
  anchor = "start",
}: {
  x: number;
  y: number;
  children: ReactNode;
  anchor?: "start" | "middle" | "end";
}) {
  return (
    <text x={x} y={y} textAnchor={anchor} fontSize={11.5} fill="var(--faint)">
      {children}
    </text>
  );
}

function Svg({
  viewBox,
  minWidth,
  label,
  children,
}: {
  viewBox: string;
  minWidth: number;
  label: string;
  children: ReactNode;
}) {
  return (
    <svg
      viewBox={viewBox}
      role="img"
      aria-label={label}
      className="block h-auto w-full"
      style={{ minWidth, fontFamily: "inherit" }}
    >
      {children}
    </svg>
  );
}

/** One deposit, end to end: who does what, and where the USDC goes. */
export function DepositFlowDiagram() {
  return (
    <Svg
      viewBox="0 0 960 330"
      minWidth={760}
      label="A deposit from creation to settlement: your server creates a request, the payer opens the link and attests a wallet, the one-time address receives USDC, and Payday settles exactly the amount to your wallet while any remainder returns to the payer."
    >
      {/* Row one: issuing and opening. */}
      <Node x={20} y={40} w={170} title="Your server" subtitle="or the dashboard" />
      <Arrow
        points={[
          [190, 68],
          [270, 68],
        ]}
        label="create"
      />
      <Node x={270} y={40} w={170} title="Payday API" subtitle="deposit request" accent="green" />
      <Arrow
        points={[
          [440, 68],
          [520, 68],
        ]}
        label="deposit_url"
      />
      <Node x={520} y={40} w={170} title="Payer" subtitle="hosted checkout" />
      <Arrow
        points={[
          [690, 68],
          [770, 68],
        ]}
        label="signs"
      />
      <Node
        x={770}
        y={40}
        w={170}
        title="One-time address"
        subtitle="bound to that wallet"
        accent="yellow"
      />

      {/* Down to the chain. */}
      <Arrow
        points={[
          [855, 96],
          [855, 150],
        ]}
      />
      <text
        x={845}
        y={128}
        textAnchor="end"
        fontSize={11.5}
        fill="var(--muted)"
        fontFamily="var(--font-mono)"
      >
        USDC, from the attested wallet
      </text>

      {/* Row two: detection and settlement. */}
      <Node x={770} y={150} w={170} title="Chain" subtitle="finalized blocks" />
      <Arrow
        points={[
          [770, 178],
          [690, 178],
        ]}
        label="indexed"
      />
      <Node
        x={520}
        y={150}
        w={170}
        title="Payday settles"
        subtitle="one transaction"
        accent="green"
      />

      <Arrow
        points={[
          [520, 178],
          [440, 178],
          [440, 250],
          [190, 250],
        ]}
      />
      <text
        x={330}
        y={242}
        textAnchor="middle"
        fontSize={11.5}
        fill={GREEN}
        fontFamily="var(--font-mono)"
      >
        exactly the amount
      </text>
      <Node x={20} y={222} w={170} title="Your wallet" subtitle="payout_address" accent="green" />

      <Arrow
        points={[
          [605, 206],
          [605, 292],
          [520, 292],
        ]}
      />
      <text
        x={640}
        y={262}
        textAnchor="start"
        fontSize={11.5}
        fill={YELLOW}
        fontFamily="var(--font-mono)"
      >
        remainder, if any
      </text>
      <Node
        x={350}
        y={264}
        w={170}
        title="Payer's wallet"
        subtitle="the attested one"
        accent="yellow"
      />

      {/* Webhooks back to the server that issued the request. */}
      <Arrow
        points={[
          [520, 162],
          [105, 162],
          [105, 96],
        ]}
        dashed
      />
      <text
        x={300}
        y={156}
        textAnchor="middle"
        fontSize={11.5}
        fill="var(--muted)"
        fontFamily="var(--font-mono)"
      >
        webhooks: ready · deposited · settled
      </text>
    </Svg>
  );
}

/** The public lifecycle, with the transitions between statuses. */
export function LifecycleDiagram() {
  const row = 60;
  return (
    <Svg
      viewBox="0 0 960 250"
      minWidth={760}
      label="Deposit request statuses: awaiting deposit leads to partially deposited or deposited, deposited leads to settled, an unmet deadline leads to expired and then returned, and needs attention pauses any request."
    >
      <Node x={20} y={row} w={170} title="awaiting_deposit" mono />
      <Arrow
        points={[
          [190, row + 28],
          [290, row + 28],
        ]}
        label="some USDC"
      />
      <Node x={290} y={row} w={190} title="partially_deposited" mono accent="blue" />
      <Arrow
        points={[
          [480, row + 28],
          [580, row + 28],
        ]}
        label="amount met"
      />
      <Node x={580} y={row} w={140} title="deposited" mono accent="blue" />
      <Arrow
        points={[
          [720, row + 28],
          [820, row + 28],
        ]}
        label="paid out"
      />
      <Node x={820} y={row} w={120} title="settled" mono accent="green" />

      {/* Straight from awaiting to deposited when one transfer covers it. */}
      <Arrow
        points={[
          [105, row],
          [105, 30],
          [650, 30],
          [650, row],
        ]}
      />
      <text
        x={378}
        y={24}
        textAnchor="middle"
        fontSize={11.5}
        fill="var(--muted)"
        fontFamily="var(--font-mono)"
      >
        one transfer covers it
      </text>

      {/* Expiry. */}
      <Arrow
        points={[
          [100, row + 56],
          [100, 180],
        ]}
      />
      <text
        x={112}
        y={152}
        textAnchor="start"
        fontSize={11.5}
        fill="var(--muted)"
        fontFamily="var(--font-mono)"
      >
        deadline passes, still short
      </text>
      <Arrow
        points={[
          [385, row + 56],
          [385, 160],
          [130, 160],
          [130, 180],
        ]}
      />
      <Node x={20} y={180} w={170} title="expired" mono />
      <Arrow
        points={[
          [190, 208],
          [330, 208],
        ]}
        label="balance sent back"
      />
      <Node x={330} y={180} w={190} title="returned" mono accent="yellow" />

      {/* Attention. */}
      <Node x={560} y={180} w={200} title="needs_attention" mono accent="yellow" />
      <Arrow
        points={[
          [650, row + 56],
          [650, 180],
        ]}
        dashed
      />
      <Caption x={770} y={212}>
        any status; paused
      </Caption>
    </Svg>
  );
}

/** How the one-time address is derived from the document and the payer's signature. */
export function AddressDerivationDiagram() {
  return (
    <Svg
      viewBox="0 0 960 300"
      minWidth={760}
      label="The issued document is canonicalized and hashed into an attribution hash; the payer's wallet signs that hash; the hash and the signature's digest make the salt; the salt and the settlement terms produce the one-time address."
    >
      <Node x={20} y={30} w={190} title="Issued document" subtitle="parties, amount, deadline…" />
      <Arrow
        points={[
          [210, 58],
          [320, 58],
        ]}
        label="canonicalize"
      />
      <Node
        x={320}
        y={30}
        w={170}
        title="Attribution hash"
        subtitle="RFC 8785 + keccak"
        accent="blue"
      />

      <Node
        x={20}
        y={130}
        w={190}
        title="Payer's wallet"
        subtitle="the one that will pay"
        accent="yellow"
      />
      <Arrow
        points={[
          [210, 158],
          [320, 158],
        ]}
        label="EIP-712 signs"
      />
      <Node
        x={320}
        y={130}
        w={170}
        title="Attestation digest"
        subtitle="hash + nonce, signed"
        accent="blue"
      />

      <Arrow
        points={[
          [490, 58],
          [595, 58],
          [595, 80],
        ]}
      />
      <Arrow
        points={[
          [490, 158],
          [595, 158],
          [595, 136],
        ]}
      />
      <Node x={520} y={80} w={150} title="Salt" subtitle="keccak(hash ‖ digest)" accent="green" />

      <Arrow
        points={[
          [670, 108],
          [740, 108],
        ]}
        label="CREATE3"
      />
      <Node
        x={740}
        y={80}
        w={200}
        title="One-time address"
        subtitle="exists before a contract"
        accent="green"
      />

      <Node
        x={520}
        y={200}
        w={420}
        h={70}
        title="Terms the address commits to"
        subtitle="token · amount · payout · deadline · recovery wallet"
      />
      <Arrow
        points={[
          [840, 200],
          [840, 136],
        ]}
      />

      <Caption x={20} y={260}>
        Anyone holding the proof can recompute every step offline.
      </Caption>
      <Caption x={20} y={278}>
        Nothing in the chain of derivation can be changed after the payer signs.
      </Caption>
    </Svg>
  );
}

/**
 * A sequence diagram: lanes across the top, messages down the page. Lanes
 * are named once; each message names its source and target by index.
 */
export interface SequenceMessage {
  from: number;
  to: number;
  label: string;
  /** A reply or a callback, drawn dashed. */
  reply?: boolean;
  /** A note under the message, in the lane it starts from. */
  note?: string;
  accent?: Accent;
}

export function Sequence({
  lanes,
  messages,
  label,
}: {
  lanes: string[];
  messages: SequenceMessage[];
  label: string;
}) {
  const width = 960;
  // Lane boxes are 160 wide, so the outer lanes sit a half box in from the edge.
  const laneGap = (width - 180) / (lanes.length - 1);
  const laneX = (index: number) => 90 + index * laneGap;
  const top = 70;
  const step = 54;
  const height = top + messages.length * step + 10;

  return (
    <Svg viewBox={`0 0 ${width} ${height}`} minWidth={720} label={label}>
      {lanes.map((lane, index) => (
        <g key={lane}>
          <rect
            x={laneX(index) - 80}
            y={12}
            width={160}
            height={36}
            rx={8}
            fill="var(--raised)"
            stroke="var(--line-strong)"
          />
          <text
            x={laneX(index)}
            y={35}
            textAnchor="middle"
            fontSize={13}
            fontWeight={500}
            fill="var(--ink)"
          >
            {lane}
          </text>
          <line
            x1={laneX(index)}
            y1={48}
            x2={laneX(index)}
            y2={height - 10}
            stroke="var(--line)"
            strokeDasharray="3 5"
          />
        </g>
      ))}
      {messages.map((message, index) => {
        const y = top + index * step;
        const x1 = laneX(message.from);
        const x2 = laneX(message.to);
        const color = message.accent ? ACCENT_STROKE[message.accent] : "var(--line-strong)";
        const self = message.from === message.to;
        return (
          <g key={index}>
            <text
              x={x1 + (x2 - x1) / 2}
              y={y - 8}
              textAnchor="middle"
              fontSize={11.5}
              fill={message.accent ? color : "var(--ink)"}
              fontFamily="var(--font-mono)"
            >
              {message.label}
            </text>
            {self ? (
              <Arrow
                points={[
                  [x1, y],
                  [x1 + 40, y],
                  [x1 + 40, y + 18],
                  [x1 + 4, y + 18],
                ]}
                color={color}
                dashed={message.reply === true}
              />
            ) : (
              <Arrow
                points={[
                  [x1 + (x2 > x1 ? 4 : -4), y],
                  [x2 + (x2 > x1 ? -4 : 4), y],
                ]}
                color={color}
                dashed={message.reply === true}
              />
            )}
            {message.note ? (
              <text
                x={x1 + (x2 - x1) / 2}
                y={y + 16}
                textAnchor="middle"
                fontSize={11}
                fill="var(--faint)"
              >
                {message.note}
              </text>
            ) : null}
          </g>
        );
      })}
    </Svg>
  );
}

/** How transfers become credits: the indexer, the ledger, and the settlement. */
export function IndexerDiagram() {
  return (
    <Svg
      viewBox="0 0 960 230"
      minWidth={760}
      label="Payday reads finalized USDC transfer logs from the chain into a durable ledger, updates each deposit request's state from that ledger, and queues eligible requests for one settlement transaction that moves the funds under the address's own terms."
    >
      <Node x={20} y={40} w={160} title="Chain" subtitle="USDC Transfer logs" />
      <Arrow
        points={[
          [180, 68],
          [300, 68],
        ]}
        label="finalized only"
      />
      <Node x={300} y={40} w={170} title="Indexer" subtitle="exact token and chain" accent="blue" />
      <Arrow
        points={[
          [470, 68],
          [550, 68],
        ]}
        label="append"
      />
      <Node
        x={550}
        y={40}
        w={180}
        title="Ledger"
        subtitle="every transfer, forever"
        accent="green"
      />
      <Arrow
        points={[
          [730, 68],
          [790, 68],
        ]}
      />
      <Node x={790} y={40} w={150} title="Request state" subtitle="status, received" />

      <Arrow
        points={[
          [640, 96],
          [640, 150],
        ]}
      />
      <text
        x={652}
        y={127}
        textAnchor="start"
        fontSize={11.5}
        fill="var(--muted)"
        fontFamily="var(--font-mono)"
      >
        eligible: amount met, or deadline passed
      </text>
      <Node
        x={550}
        y={150}
        w={180}
        title="Settlement"
        subtitle="one batched transaction"
        accent="green"
      />
      <Arrow
        points={[
          [550, 178],
          [480, 178],
        ]}
      />
      <Node
        x={280}
        y={150}
        w={200}
        title="Deposit contract"
        subtitle="terms fixed in the address"
        accent="yellow"
      />
      <Arrow
        points={[
          [280, 178],
          [220, 178],
        ]}
      />
      <Node
        x={20}
        y={150}
        w={200}
        title="Payout and returns"
        subtitle="amount → you · rest → payer"
      />
    </Svg>
  );
}
