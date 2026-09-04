/**
 * A stand-in for the Payday API, so the checkout's states and the dashboard's
 * flows can be driven deterministically in a browser.
 *
 * Payer scenarios are encoded in the payment id, which keeps every test
 * independent — no shared mutable state, no ordering between specs. Shapes
 * mirror `PayerPaymentResponse` and `PaymentResponse` in
 * crates/gateway-core/src/dto.rs exactly.
 *
 * For the dashboard the same process plays the merchant API behind a fake
 * bearer check, the presigned upload target, and the OTP issuer
 * (`/passwordless/start`, `/oauth/token`). Merchant state is in memory and
 * seeded once; specs create their own records and never depend on each other's.
 */
import { createHash, randomUUID } from "node:crypto";
import { createServer } from "node:http";

const PORT = Number(process.env.STUB_PORT ?? 4010);
const ORIGIN = `http://127.0.0.1:${PORT}`;
/** Where the stand-in identity provider sends the payer back to. */
const CHECKOUT_ORIGIN = process.env.CHECKOUT_ORIGIN ?? "http://127.0.0.1:3003";
const TOKEN = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
const ADDRESS = "0x9a3f0000000000000000000000000000000000c2";
const FACTORY = "0x5FbDB2315678afecb367f032d93F642f64180aa3";
const RECOVERY = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";
const PAYOUT = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const SETTLEMENT_TX = "0x00210b337281f97a1d0747a1535795822998906f5f7e89917a8cd4ee83aa0190";
const ATTESTOR = "0x976EA74026E726554dB657fA54763abd0C3a0aa9";

/** What the fake issuer hands out, and what the merchant routes accept. */
const DASHBOARD_TOKEN = "stub-dashboard-token";
const OTP = "123456";

const PDF_BYTES = Buffer.from(
  "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n",
);

// A real QR is unnecessary here; only load success or a 410 matters.
const QR_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="224" height="224" viewBox="0 0 2 2">' +
  '<rect width="2" height="2" fill="#fff"/><rect width="1" height="1" fill="#090811"/></svg>';

const GATED = new Set(["verified_email", "verified_identity", "verified_identity_unattributed"]);

/** `VerificationRequirementsResponse::for_mode` */
function requirements(mode, completed = false) {
  const required = completed ? "approved" : "pending";
  const identity = mode === "verified_identity" || mode === "verified_identity_unattributed";
  return {
    email: GATED.has(mode) ? required : "not_required",
    document: identity ? required : "not_required",
    liveness: identity ? required : "not_required",
    identity_match: mode === "verified_identity" ? required : "not_required",
    complete: !GATED.has(mode) || completed,
  };
}

const ATTACHMENT = {
  id: "0198f80c-8d2f-7dc1-a369-90556a64f7aa",
  filename: "INV-1042.pdf",
  mime_type: "application/pdf",
  byte_length: "48211",
  sha256: "0x9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
};

/* ------------------------------------------------------------------------ */
/* Payer scenarios                                                           */
/* ------------------------------------------------------------------------ */

function base(overrides = {}) {
  const now = Math.floor(Date.now() / 1000);
  return {
    id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
    issuer_name: "Acme Corp",
    heading: null,
    payer_policy: { mode: "permissionless", expected_email_hint: null },
    requirements: requirements("permissionless"),
    status: "awaiting_payment",
    payable: true,
    expires_at: new Date((now + 3600) * 1000).toISOString(),
    server_timestamp: String(now),
    settlement_tx_hash: null,
    settlement_explorer_url: null,
    payer_message: null,
    content_unlocked: true,
    chain: { id: "143", name: "Monad" },
    token: { symbol: "USDC", address: TOKEN, decimals: 6 },
    amount: "25.000000",
    amount_base_units: "25000000",
    received: "0",
    received_base_units: "0",
    remaining: "25.000000",
    remaining_base_units: "25000000",
    address: ADDRESS,
    address_explorer_url: null,
    payment_uri: `ethereum:${TOKEN}@143/transfer?address=${ADDRESS}&uint256=25000000`,
    invoice: {
      amount: "25.000000",
      amount_base_units: "25000000",
      bill_to: { name: "Globex Corporation" },
      notes: null,
      reference: null,
      attachment: null,
    },
    ...overrides,
  };
}

/**
 * Payer sessions minted by the verification routes:
 * token -> { id, emailVerified, identity: null | { status, attempt_number, retry_available } }.
 * A session is good for exactly the payment it was started on.
 */
const sessions = new Map();

/**
 * Hosted identity sessions of the stand-in provider: hostedId -> session
 * token. The hosted page decides per scenario and sends the payer back to
 * the checkout with whatever a real provider would append, which the page
 * must ignore.
 */
const hosted = new Map();

const IDENTITY_MODES = new Set(["verified_identity", "verified_identity_unattributed"]);

/** What a scenario's hosted check decides when the payer returns. */
function decideHosted(scenarioId, session) {
  const attempt = session.identity?.attempt_number ?? 1;
  if (scenarioId === "gated-identity-declined") {
    return attempt >= 2
      ? { status: "review_required", attempt_number: attempt, retry_available: false }
      : { status: "declined", attempt_number: attempt, retry_available: true };
  }
  return { status: "approved", attempt_number: attempt, retry_available: false };
}

/** The session a request presents, if it is valid for `id`. */
function sessionFor(req, id) {
  const token = req.headers["payday-payer-session"];
  const session = token ? sessions.get(token) : undefined;
  return session && session.id === id ? session : null;
}

/** A gated invoice as its verifying session sees it. */
function gatedFor(mode, session) {
  if (!session?.emailVerified) return locked(mode);
  if (mode === "verified_email" || session.identity?.status === "approved") {
    return base({
      heading: "Consulting — August",
      payer_policy: { mode, expected_email_hint: "a****@e***.com" },
      requirements: requirements(mode, true),
      invoice: {
        ...base().invoice,
        bill_to: { name: "Globex Corporation" },
        reference: "INV-1042",
        notes: "Net 30",
        attachment: ATTACHMENT,
      },
    });
  }
  // Identity modes: the mailbox is proven, the identity facts are not.
  const declined =
    session.identity?.status === "declined" || session.identity?.status === "review_required";
  return locked(mode, {
    ...requirements(mode),
    email: "approved",
    ...(declined ? { document: "declined" } : {}),
  });
}

/** The `/verify` shape for one session. */
function verifyStatus(payment, session) {
  const mode = payment.payer_policy.mode;
  const identity = session?.identity ?? null;
  const startable =
    IDENTITY_MODES.has(mode) &&
    Boolean(session?.emailVerified) &&
    !payment.requirements.complete &&
    identity?.status !== "review_required" &&
    identity?.status !== "in_review";
  return {
    requirements: payment.requirements,
    identity_start_available: startable,
    identity,
  };
}

/** A gated invoice before verification: only the issuer, heading, and policy leave the API. */
/**
 * A merchant-issued payment as its payer sees it: everything withheld while
 * the policy is gated, since this projection carries no payer session.
 */
function projectForPayer(payment) {
  const mode = payment.payer_policy.mode;
  const gated = GATED.has(mode);
  const shared = {
    id: payment.id,
    issuer_name: payment.issuer.name,
    heading: payment.heading,
    payer_policy: { mode, expected_email_hint: gated ? "a****@e***.com" : null },
    requirements: requirements(mode, Boolean(payment.verification_completed_at)),
    status: payment.status,
    payable: payment.status === "awaiting_payment" || payment.status === "partially_paid",
    expires_at: payment.expires_at,
    server_timestamp: String(Math.floor(Date.now() / 1000)),
    settlement_tx_hash: payment.settlement_tx_hash ?? null,
    settlement_explorer_url: payment.settlement_explorer_url ?? null,
    payer_message: null,
  };
  if (gated && !payment.verification_completed_at) {
    return {
      ...shared,
      content_unlocked: false,
      chain: null,
      token: null,
      amount: null,
      amount_base_units: null,
      received: null,
      received_base_units: null,
      remaining: null,
      remaining_base_units: null,
      address: null,
      address_explorer_url: null,
      payment_uri: null,
      invoice: null,
    };
  }
  return {
    ...shared,
    content_unlocked: true,
    chain: payment.chain,
    token: payment.token,
    amount: payment.amount,
    amount_base_units: payment.amount_base_units,
    received: payment.received,
    received_base_units: payment.received_base_units,
    remaining: payment.remaining,
    remaining_base_units: payment.remaining_base_units,
    address: payment.address,
    address_explorer_url: payment.address_explorer_url ?? null,
    payment_uri: `ethereum:${payment.token.address}@${payment.chain.id}/transfer?address=${payment.address}&uint256=${payment.remaining_base_units}`,
    invoice: {
      amount: payment.amount,
      amount_base_units: payment.amount_base_units,
      bill_to: payment.bill_to,
      notes: payment.notes,
      reference: payment.reference,
      attachment: payment.attachment,
    },
  };
}

function locked(mode, facts = requirements(mode)) {
  return base({
    heading: "Consulting — August",
    payer_policy: { mode, expected_email_hint: "a****@e***.com" },
    requirements: facts,
    content_unlocked: false,
    chain: null,
    token: null,
    amount: null,
    amount_base_units: null,
    received: null,
    received_base_units: null,
    remaining: null,
    remaining_base_units: null,
    address: null,
    address_explorer_url: null,
    payment_uri: null,
    invoice: null,
  });
}

const PARTIAL = {
  status: "partially_paid",
  received: "10.000000",
  received_base_units: "10000000",
  remaining: "15.000000",
  remaining_base_units: "15000000",
  payment_uri: `ethereum:${TOKEN}@143/transfer?address=${ADDRESS}&uint256=15000000`,
};

const FULL = {
  received: "25.000000",
  received_base_units: "25000000",
  remaining: "0",
  remaining_base_units: "0",
};

const CLOSED = { payable: false, payment_uri: null };

const SETTLED = {
  status: "settled",
  settlement_tx_hash: SETTLEMENT_TX,
  settlement_explorer_url: `https://monadvision.com/tx/${SETTLEMENT_TX}`,
};

/** Reads counted per id, so one scenario can change between polls. */
const reads = new Map();

const scenarios = {
  awaiting: () => base(),
  // The full document: heading, reference, notes, and an attached PDF.
  invoice: () =>
    base({
      heading: "Consulting — August",
      invoice: {
        ...base().invoice,
        bill_to: { name: "Globex Corporation", email: "ap@globex.example", details: "PO 7781" },
        reference: "INV-1042",
        notes: "Net 30. Thank you for your business.",
        attachment: ATTACHMENT,
      },
    }),
  "gated-email": (id, session) => gatedFor("verified_email", session),
  "gated-identity": (id, session) => gatedFor("verified_identity", session),
  "gated-unattributed": (id, session) => gatedFor("verified_identity_unattributed", session),
  // The hosted check declines the first attempt and stops on the second.
  "gated-identity-declined": (id, session) => gatedFor("verified_identity", session),
  partial: () => base(PARTIAL),
  paid: () => base({ ...FULL, ...CLOSED, status: "paid" }),
  settled: () => base({ ...FULL, ...CLOSED, ...SETTLED }),
  // More arrived than was asked for. Settlement is exact, so the merchant got
  // 25 and the remainder went to the recovery wallet; the receipt says so.
  "settled-overpaid": () =>
    base({
      ...FULL,
      ...CLOSED,
      ...SETTLED,
      received: "30.000000",
      received_base_units: "30000000",
    }),
  expired: () => base({ ...CLOSED, status: "expired" }),
  "expired-funded": () => base({ ...PARTIAL, ...CLOSED, status: "expired" }),
  returned: () => base({ ...PARTIAL, ...CLOSED, status: "returned" }),
  attention: () =>
    base({
      ...CLOSED,
      status: "needs_attention",
      payer_message:
        "Payout is paused, but your funds remain safe. The merchant and Payday support are resolving settlement; do not send a second payment.",
    }),
  // Payable false while the status has not yet flipped: the deadline passed but
  // the indexer has not committed the expiry.
  closing: () => base(CLOSED),
  // Ends moments from now, to prove the page waits for the server rather than
  // declaring the payment expired off its own countdown.
  ending: () => {
    const now = Math.floor(Date.now() / 1000);
    return base({
      server_timestamp: String(now),
      expires_at: new Date((now + 3) * 1000).toISOString(),
    });
  },
  // First read is awaiting; every later read is partially paid.
  transition: (id) => {
    const seen = (reads.get(id) ?? 0) + 1;
    reads.set(id, seen);
    return seen <= 1 ? base() : base(PARTIAL);
  },
  // A different chain, to prove the wallet button refuses to sign for it.
  "other-chain": () => base({ chain: { id: "1", name: "Ethereum" } }),
};

function scenarioFor(id) {
  return scenarios[id.replace(/^pay_/, "")] ?? null;
}

/* ------------------------------------------------------------------------ */
/* Merchant state                                                            */
/* ------------------------------------------------------------------------ */

const VERIFICATION_FILTERS = ["not_required", "pending", "verified", "likely_unsolicited"];

const store = {
  customers: new Map(),
  /** account key -> { issuers, payoutAddresses, issuerAddresses } */
  issuerWorlds: new Map(),
  /** id -> { id, filename, bytes, finalizeCalls, status, descriptor } */
  attachments: new Map(),
  /** id -> full PaymentResponse */
  payments: new Map(),
};

const DECIMALS = 6;

/** "25.5" -> "25500000"; null for anything that is not a positive decimal. */
function toBaseUnits(amount) {
  const match = /^(\d+)(?:\.(\d{1,6}))?$/.exec(String(amount ?? "").trim());
  if (!match) return null;
  const units =
    BigInt(match[1]) * 10n ** BigInt(DECIMALS) + BigInt((match[2] ?? "").padEnd(DECIMALS, "0"));
  return units > 0n ? units.toString() : null;
}

function fromBaseUnits(units) {
  const value = BigInt(units);
  const whole = value / 10n ** BigInt(DECIMALS);
  const fraction = (value % 10n ** BigInt(DECIMALS)).toString().padStart(DECIMALS, "0");
  return `${whole}.${fraction}`;
}

function hex32(seed) {
  return `0x${createHash("sha256").update(seed).digest("hex")}`;
}

function merchantPayment(input, extra = {}) {
  const id = extra.id ?? `pay_${randomUUID()}`;
  const created = extra.created_at ?? new Date().toISOString();
  const amountUnits = toBaseUnits(input.amount);
  // A chosen moment wins over a duration, as it does at the API; the response
  // still reports the lifetime it works out to.
  const expiresIn = input.expires_at
    ? Math.round((Date.parse(input.expires_at) - Date.parse(created)) / 1000)
    : (input.expires_in ?? 86_400);
  const receivedUnits = extra.received_base_units ?? "0";
  const remainingUnits = (
    BigInt(amountUnits) > BigInt(receivedUnits) ? BigInt(amountUnits) - BigInt(receivedUnits) : 0n
  ).toString();
  const address = `0x${createHash("sha256").update(id).digest("hex").slice(0, 40)}`;
  return {
    id,
    payment_url: `http://127.0.0.1:3003/pay/${id}`,
    address,
    address_explorer_url: null,
    payout_address: input.payout_address,
    recovery_address: RECOVERY,
    expires_at: new Date(Date.parse(created) + expiresIn * 1000).toISOString(),
    expires_in: expiresIn,
    amount: fromBaseUnits(amountUnits),
    amount_base_units: amountUnits,
    received: fromBaseUnits(receivedUnits),
    received_base_units: receivedUnits,
    remaining: fromBaseUnits(remainingUnits),
    remaining_base_units: remainingUnits,
    currency: "USDC",
    fee_amount: "0.000000",
    fee_amount_base_units: "0",
    net_amount: fromBaseUnits(amountUnits),
    net_amount_base_units: amountUnits,
    status: "awaiting_payment",
    token: { symbol: "USDC", address: TOKEN, decimals: 6 },
    chain: { id: "143", name: "Monad" },
    settlement_tx_hash: null,
    settlement_explorer_url: null,
    settled_at: null,
    settled_block: null,
    self_settlement: { factory: FACTORY, salt: hex32(`salt:${id}`) },
    attention: null,
    issuer: input.issuer,
    bill_to: input.bill_to,
    notes: input.notes ?? null,
    heading: input.heading ?? null,
    issuer_id: input.issuer_id ?? null,
    reference: input.reference ?? null,
    customer_id: input.customer_id ?? null,
    payer_policy: input.payer_policy,
    attachment: extra.attachment ?? null,
    verification_completed_at: null,
    likely_unsolicited_at: null,
    verification_attempts: [],
    attribution: { version: 1, hash: hex32(`attribution:${id}`) },
    metadata: input.metadata ?? {},
    created_at: created,
    updated_at: created,
    paid_at: null,
    paid_at_block: null,
    expired_at: null,
    cancellation_requested_at: null,
    transfers: [],
    indexer_freshness: {
      last_indexed_block: "1000",
      last_finalized_block: "1000",
      cursor_updated_at: created,
    },
    as_of: { block: "1000", at: created },
    ...extra,
  };
}

/** The merchant's verification view; seeded invoices carry their attempts. */
function verificationDetail(payment) {
  const mode = payment.payer_policy.mode;
  const completed = payment.verification_completed_at !== null;
  const attempts = payment.verification_attempts ?? [];
  const latest = [...attempts].reverse().find((attempt) => attempt.kind === "identity") ?? null;
  const reviewed = attempts.some((attempt) => attempt.review !== null);
  return {
    payer_policy_mode: mode,
    verification_completed_at: payment.verification_completed_at,
    likely_unsolicited_at: payment.likely_unsolicited_at,
    facts: completed
      ? requirements(mode, true)
      : {
          ...requirements(mode),
          email: attempts.some(
            (attempt) => attempt.kind === "email" && attempt.status === "approved",
          )
            ? "approved"
            : requirements(mode).email,
          ...(latest && ["declined", "review_required"].includes(latest.status)
            ? { document: latest.document }
            : {}),
        },
    attempts,
    review_available: latest !== null && ["declined", "review_required"].includes(latest.status),
    retry_available:
      !completed &&
      !reviewed &&
      latest !== null &&
      ["declined", "expired", "abandoned"].includes(latest.status),
  };
}

function summary(payment) {
  return {
    id: payment.id,
    heading: payment.heading,
    bill_to_name: payment.bill_to.name,
    issuer_id: payment.issuer_id ?? null,
    reference: payment.reference,
    metadata: payment.metadata,
    payer_policy_mode: payment.payer_policy.mode,
    customer_id: payment.customer_id,
    has_attachment: payment.attachment !== null,
    verification_completed_at: payment.verification_completed_at,
    likely_unsolicited_at: payment.likely_unsolicited_at,
    created_at: payment.created_at,
    status: payment.status,
    amount: payment.amount,
    received: payment.received,
    cancellation_requested_at: payment.cancellation_requested_at,
  };
}

function proofFor(payment) {
  const transfers = payment.transfers
    .filter((transfer) => transfer.disposition === "credited")
    .map((transfer) => ({
      transaction_hash: transfer.transaction_hash,
      sender: transfer.sender,
      recipient: payment.address,
      amount_base_units: transfer.amount_base_units,
      block_number: transfer.block,
    }));
  const gated = GATED.has(payment.payer_policy.mode);
  return {
    version: "payday.proof.v1",
    payment_id: payment.id,
    canonical_issuance_snapshot: {
      schema: "payday.invoice",
      canonicalization: "RFC8785",
      issuer: payment.issuer,
      bill_to: payment.bill_to,
      amount_base_units: payment.amount_base_units,
      notes: payment.notes,
      heading: payment.heading,
      reference: payment.reference,
      expiration_timestamp: String(Math.floor(Date.parse(payment.expires_at) / 1000)),
      payer_policy: payment.payer_policy,
      attachment: payment.attachment
        ? {
            id: payment.attachment.id,
            byte_length: payment.attachment.byte_length,
            sha256: payment.attachment.sha256,
          }
        : null,
      chain_id: "143",
      token_address: TOKEN,
      receiver_address: payment.payout_address,
      recovery_address: RECOVERY,
      factory_address: FACTORY,
    },
    canonicalization: "RFC8785",
    attribution_nonce: hex32(`nonce:${payment.id}`),
    attribution_hash: payment.attribution.hash,
    salt: payment.self_settlement.salt,
    chain_id: "143",
    factory_address: FACTORY,
    payment_address: payment.address,
    token_address: TOKEN,
    settlement_transaction_hash: payment.settlement_tx_hash,
    transfers,
    verification: {
      payload: {
        version: "payday.attestation.v1",
        payment_id: payment.id,
        payer_policy_mode: payment.payer_policy.mode,
        result: gated ? "approved" : "not_required",
        verified_at: payment.verification_completed_at,
      },
      signer: ATTESTOR,
      signature: `0x${"ab".repeat(65)}`,
    },
  };
}

function seed() {
  const customer = {
    id: "0198f80c-8d2f-7dc1-a369-90556a64f7c1",
    name: "Globex Corporation",
    email: "ap@globex.example",
    details: "PO 7781",
    created_at: "2026-08-01T09:00:00.000Z",
    updated_at: "2026-08-01T09:00:00.000Z",
  };
  store.customers.set(customer.id, customer);

  const settledAt = "2026-08-20T12:00:00.000Z";
  const settled = merchantPayment(
    {
      amount: "25",
      payout_address: PAYOUT,
      issuer: { name: "Acme Corp", email: "billing@acme.example" },
      bill_to: { name: "Globex Corporation", email: "ap@globex.example" },
      heading: "Consulting — August",
      reference: "INV-1042",
      notes: "Net 30. Thank you for your business.",
      customer_id: customer.id,
      payer_policy: { mode: "verified_email", expected_email: "alice@globex.example" },
    },
    {
      id: "pay_seed-settled",
      created_at: "2026-08-18T10:00:00.000Z",
      status: "settled",
      received_base_units: "30000000",
      attachment: ATTACHMENT,
      verification_completed_at: "2026-08-19T08:30:00.000Z",
      paid_at: settledAt,
      paid_at_block: "1200",
      settled_at: settledAt,
      settled_block: "1201",
      settlement_tx_hash: SETTLEMENT_TX,
      settlement_explorer_url: `https://monadvision.com/tx/${SETTLEMENT_TX}`,
      transfers: [
        {
          transaction_hash: hex32("tx:credited"),
          explorer_url: null,
          sender: "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
          amount: "30.000000",
          amount_base_units: "30000000",
          block: "1200",
          timestamp: "2026-08-19T09:00:00.000Z",
          disposition: "credited",
          collected: true,
        },
        {
          transaction_hash: hex32("tx:late"),
          explorer_url: null,
          sender: "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
          amount: "5.000000",
          amount_base_units: "5000000",
          block: "1300",
          timestamp: "2026-08-21T09:00:00.000Z",
          disposition: "late",
          collected: true,
        },
      ],
    },
  );
  store.payments.set(settled.id, settled);

  // Funds arrived before the expected payer verified: the row is flagged, the
  // address is not quarantined.
  const unsolicited = merchantPayment(
    {
      amount: "40",
      payout_address: PAYOUT,
      issuer: { name: "Acme Corp" },
      bill_to: { name: "Initech" },
      heading: "Retainer — September",
      reference: "INV-1043",
      payer_policy: {
        mode: "verified_identity",
        expected_email: "bob@initech.example",
        expected_identity: { first_name: "Bob", last_name: "Slydell" },
      },
    },
    {
      id: "pay_seed-unsolicited",
      created_at: "2026-08-25T10:00:00.000Z",
      status: "paid",
      received_base_units: "40000000",
      likely_unsolicited_at: "2026-08-26T11:00:00.000Z",
      paid_at: "2026-08-26T11:00:00.000Z",
      paid_at_block: "1400",
      verification_attempts: [
        {
          id: "0198f80c-8d2f-7dc1-a369-90556a64f7e1",
          kind: "email",
          status: "approved",
          provider: "auth0",
          provider_reference: null,
          attempt_number: 1,
          document: "not_required",
          liveness: "not_required",
          identity_match: "not_required",
          risk_codes: [],
          country_code: null,
          verified_at: "2026-08-26T10:30:00.000Z",
          expires_at: null,
          created_at: "2026-08-26T10:29:00.000Z",
          review: null,
        },
        {
          id: "0198f80c-8d2f-7dc1-a369-90556a64f7e2",
          kind: "identity",
          status: "declined",
          provider: "didit",
          provider_reference: "9f1c0f6e-1111-4c1a-9c1e-000000000003",
          attempt_number: 1,
          document: "declined",
          liveness: "approved",
          identity_match: "pending",
          risk_codes: ["EXPECTED_DETAILS_MISMATCH", "DOCUMENT_EXPIRED"],
          country_code: "ESP",
          verified_at: null,
          expires_at: null,
          created_at: "2026-08-26T10:40:00.000Z",
          review: null,
        },
      ],
      transfers: [
        {
          transaction_hash: hex32("tx:unsolicited"),
          explorer_url: null,
          sender: "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65",
          amount: "40.000000",
          amount_base_units: "40000000",
          block: "1400",
          timestamp: "2026-08-26T11:00:00.000Z",
          disposition: "credited",
          collected: false,
        },
      ],
    },
  );
  store.payments.set(unsolicited.id, unsolicited);
}

seed();

/* ------------------------------------------------------------------------ */
/* HTTP plumbing                                                             */
/* ------------------------------------------------------------------------ */

const CORS = {
  "access-control-allow-origin": "*",
  // The same list gatewayd allows the dashboard origin, so a method the real
  // API would refuse at the preflight is not silently fine here.
  "access-control-allow-methods": "GET, POST, PATCH, PUT, DELETE, OPTIONS",
  "access-control-allow-headers":
    "authorization, content-type, accept, idempotency-key, payday-payer-session, x-amz-tagging",
  "access-control-max-age": "600",
};

function send(res, status, body, headers = {}) {
  res.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
    ...CORS,
    ...headers,
  });
  res.end(typeof body === "string" ? body : JSON.stringify(body));
}

function sendBytes(res, status, bytes, contentType, headers = {}) {
  res.writeHead(status, {
    "content-type": contentType,
    "content-length": String(bytes.length),
    "cache-control": "no-store",
    ...CORS,
    ...headers,
  });
  res.end(bytes);
}

function fail(res, status, code, message) {
  return send(res, status, {
    error: { code, message },
    request_id: `stub-${randomUUID().slice(0, 8)}`,
  });
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
}

async function readJson(req) {
  const raw = await readBody(req);
  if (raw.length === 0) return {};
  return JSON.parse(raw.toString("utf8"));
}

function authorized(req) {
  return String(req.headers.authorization ?? "").startsWith(`Bearer ${DASHBOARD_TOKEN}`);
}

/** Which merchant a request speaks for; the bare token is one account too. */
function accountKey(req) {
  return String(req.headers.authorization ?? "").slice(`Bearer ${DASHBOARD_TOKEN}`.length);
}

/** That account's issuer world, created on first sight. */
function issuerWorld(req) {
  const key = accountKey(req);
  let world = store.issuerWorlds.get(key);
  if (!world) {
    world = { issuers: new Map(), payoutAddresses: new Map(), issuerAddresses: new Map() };
    store.issuerWorlds.set(key, world);
  }
  return world;
}

function paginate(items, params) {
  const limit = Math.min(Math.max(Number(params.get("limit") ?? 20), 1), 100);
  const after = params.get("starting_after");
  let start = 0;
  if (after) {
    const index = items.findIndex((item) => item.id === after);
    start = index === -1 ? items.length : index + 1;
  }
  const page = items.slice(start, start + limit);
  const next = start + limit < items.length ? (page[page.length - 1]?.id ?? null) : null;
  return { page, next };
}

function customerFrom(body, existing) {
  const name = typeof body.name === "string" ? body.name.trim() : "";
  if (!name || name.length > 255) throw new Error("name is required");
  const optional = (value) => (typeof value === "string" && value.trim() ? value.trim() : null);
  const now = new Date().toISOString();
  return {
    id: existing?.id ?? randomUUID(),
    name,
    email: optional(body.email),
    details: optional(body.details),
    created_at: existing?.created_at ?? now,
    updated_at: now,
  };
}

/* ------------------------------------------------------------------------ */
/* Routes                                                                    */
/* ------------------------------------------------------------------------ */

async function payer(req, res, url) {
  const match = url.pathname.match(
    /^\/v1\/payer\/payments\/([^/]+)(\/qr|\/attachment|\/verify|\/verify\/email\/start|\/verify\/email\/confirm|\/verify\/identity\/start)?$/,
  );
  if (!match) return false;
  const write =
    match[2] === "/verify/email/start" ||
    match[2] === "/verify/email/confirm" ||
    match[2] === "/verify/identity/start";
  if (req.method !== (write ? "POST" : "GET")) {
    return fail(res, 405, "method_not_allowed", "method not allowed");
  }

  const id = decodeURIComponent(match[1]);
  const scenario = scenarioFor(id);
  // The scenarios drive the checkout specs; anything else the merchant side
  // issued is projected from the store, as the real payer route serves any
  // payment rather than a fixed cast.
  const issued = scenario ? null : store.payments.get(id);
  if (!scenario && !issued) {
    return fail(res, 401, "invalid_payment_link", "Payment link is not valid");
  }

  const session = sessionFor(req, id);
  const payment = scenario ? { ...scenario(id, session), id } : { ...projectForPayer(issued), id };
  const mode = payment.payer_policy.mode;

  if (match[2] === "/verify/email/start") {
    if (!GATED.has(mode)) return fail(res, 409, "verification_not_required", "Nothing to verify");
    // The code goes to the merchant's asserted mailbox; the request names none.
    const reused = session ?? null;
    const token = reused ? req.headers["payday-payer-session"] : `pps_${randomUUID()}`;
    if (!reused) sessions.set(token, { id, emailVerified: false, identity: null });
    return send(res, 200, {
      payer_session: token,
      expires_at: new Date(Date.now() + 24 * 3600 * 1000).toISOString(),
    });
  }

  if (match[2] === "/verify/email/confirm") {
    if (!GATED.has(mode)) return fail(res, 409, "verification_not_required", "Nothing to verify");
    if (!session) return fail(res, 401, "payer_session_invalid", "Start verification again");
    const body = await readJson(req);
    if (body.otp !== OTP) return fail(res, 401, "otp_invalid", "The code was not accepted");
    session.emailVerified = true;
    const current = { ...scenario(id, session), id };
    return send(res, 200, verifyStatus(current, session));
  }

  if (match[2] === "/verify/identity/start") {
    if (!IDENTITY_MODES.has(mode)) {
      return fail(res, 409, "verification_not_required", "Nothing to verify");
    }
    if (!session) return fail(res, 401, "payer_session_invalid", "Start verification again");
    if (!session.emailVerified) {
      return fail(res, 409, "email_verification_required", "Verify the email first");
    }
    if (session.identity?.status === "approved")
      return send(res, 200, { outcome: { type: "reused" } });
    if (session.identity?.status === "review_required") {
      return fail(res, 409, "review_required", "A human review is required");
    }
    // The unattributed scenario holds an earlier credential of this merchant.
    if (id.replace(/^pay_/, "") === "gated-unattributed") {
      session.identity = { status: "approved", attempt_number: 1, retry_available: false };
      return send(res, 200, { outcome: { type: "reused" } });
    }
    const attempt = (session.identity?.attempt_number ?? 0) + 1;
    session.identity = { status: "pending", attempt_number: attempt, retry_available: false };
    const hostedId = randomUUID();
    hosted.set(hostedId, req.headers["payday-payer-session"]);
    return send(res, 200, { outcome: { type: "redirect", url: `${ORIGIN}/__didit/${hostedId}` } });
  }

  if (match[2] === "/verify") {
    if (req.headers["payday-payer-session"] && !session) {
      return fail(res, 401, "payer_session_invalid", "Start verification again");
    }
    return send(res, 200, verifyStatus(payment, session));
  }

  if (match[2] === "/qr") {
    if (!payment.content_unlocked) {
      return fail(res, 401, "verification_required", "Verify to view this invoice");
    }
    if (!payment.payable) return fail(res, 410, "payment_not_payable", "No longer payable");
    return sendBytes(res, 200, Buffer.from(QR_SVG), "image/svg+xml; charset=utf-8");
  }

  if (match[2] === "/attachment") {
    if (!payment.content_unlocked) {
      return fail(res, 401, "verification_required", "Verify to view this invoice");
    }
    const attachment = payment.invoice?.attachment;
    if (!attachment) return fail(res, 404, "attachment_not_found", "No attachment");
    return send(res, 200, {
      ...attachment,
      download_url: `${ORIGIN}/__download/${attachment.id}.pdf`,
    });
  }

  return send(res, 200, payment);
}

async function issuer(req, res, url) {
  if (url.pathname === "/passwordless/start") {
    if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
    const body = await readJson(req);
    if (body.client_id !== "payday-dashboard-local" || !body.email) {
      return send(res, 400, {
        error: "bad.request",
        error_description: "client_id and email are required",
      });
    }
    return send(res, 200, { _id: "stub", email: body.email, email_verified: false });
  }
  if (url.pathname === "/oauth/token") {
    if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
    const body = await readJson(req);
    if (body.otp !== OTP || body.client_id !== "payday-dashboard-local") {
      return send(res, 403, {
        error: "invalid_grant",
        error_description: "Wrong email or verification code.",
      });
    }
    // The token carries the mailbox it was minted for, so issuer identities
    // are per-merchant here as they are in the API: specs that need an empty
    // account and specs that need a set-up one can run side by side.
    return send(res, 200, {
      access_token: `${DASHBOARD_TOKEN}.${String(body.username ?? "").replace(/[^a-z0-9]+/gi, "-")}`,
      token_type: "Bearer",
      expires_in: 300,
      scope: "openid",
    });
  }
  return false;
}

/**
 * The stand-in identity provider's hosted session: decides at once and sends
 * the payer back to the checkout with the sort of query string a real
 * provider appends. Nothing in it is trusted by the page.
 */
function identityProvider(req, res, url) {
  const match = url.pathname.match(/^\/__didit\/([^/]+)$/);
  if (!match) return false;
  const token = hosted.get(match[1]);
  const session = token ? sessions.get(token) : undefined;
  if (!session) return fail(res, 404, "not_found", "unknown hosted session");
  hosted.delete(match[1]);
  session.identity = decideHosted(session.id.replace(/^pay_/, ""), session);
  res.writeHead(302, {
    location: `${CHECKOUT_ORIGIN}/pay/${encodeURIComponent(session.id)}?status=Approved&session_id=${match[1]}`,
  });
  res.end();
  return true;
}

/** The presigned PUT target and the signed download URL, standing in for S3. */
async function objectStore(req, res, url) {
  const upload = url.pathname.match(/^\/__upload\/([^/]+)$/);
  if (upload) {
    if (req.method !== "PUT") return fail(res, 405, "method_not_allowed", "method not allowed");
    const draft = store.attachments.get(upload[1]);
    if (!draft) return sendBytes(res, 404, Buffer.from("NoSuchUpload"), "text/plain");
    // A presigned URL signs these two headers; S3 refuses a PUT without them.
    if (
      req.headers["content-type"] !== "application/pdf" ||
      req.headers["x-amz-tagging"] !== "payday-upload=pending"
    ) {
      return sendBytes(res, 403, Buffer.from("SignatureDoesNotMatch"), "text/plain");
    }
    draft.bytes = await readBody(req);
    return sendBytes(res, 200, Buffer.alloc(0), "text/plain", { etag: '"stub"' });
  }

  const download = url.pathname.match(/^\/__download\/([^/]+)\.pdf$/);
  if (download) {
    const stored = store.attachments.get(download[1]);
    return sendBytes(res, 200, stored?.bytes ?? PDF_BYTES, "application/pdf", {
      "content-disposition": `attachment; filename="${stored?.filename ?? "invoice.pdf"}"`,
    });
  }
  return false;
}

/**
 * Mirrors gatewayd's `customer_stats`: how many requests, how much has
 * actually been confirmed across all of them, and how much remains on the
 * ones still open. Base units — `payment.amount`/`.received` here are
 * display decimal strings, so scale by the token's decimals to match.
 */
function customerStats(customerId) {
  const own = [...store.payments.values()].filter(
    (payment) => payment.customer_id === customerId,
  );
  let collected = 0;
  let pending = 0;
  for (const payment of own) {
    collected += Number(payment.received);
    if (payment.status === "awaiting_payment" || payment.status === "partially_paid") {
      pending += Number(payment.amount) - Number(payment.received);
    }
  }
  const baseUnits = (decimal) => Math.round(decimal * 1_000_000).toString();
  return {
    request_count: own.length,
    collected_base_units: baseUnits(collected),
    pending_base_units: baseUnits(pending),
  };
}

async function customers(req, res, url) {
  if (url.pathname === "/v1/customers") {
    if (req.method === "POST") {
      let record;
      try {
        record = customerFrom(await readJson(req));
      } catch (error) {
        return fail(res, 400, "invalid_request", String(error.message ?? error));
      }
      store.customers.set(record.id, record);
      return send(res, 201, record);
    }
    if (req.method === "GET") {
      const all = [...store.customers.values()].sort((a, b) =>
        b.created_at.localeCompare(a.created_at),
      );
      const { page, next } = paginate(all, url.searchParams);
      return send(res, 200, { customers: page, next_cursor: next });
    }
    return fail(res, 405, "method_not_allowed", "method not allowed");
  }

  const match = url.pathname.match(/^\/v1\/customers\/([^/]+)$/);
  if (!match) return false;
  const existing = store.customers.get(decodeURIComponent(match[1]));
  if (!existing) return fail(res, 404, "customer_not_found", "No such customer");
  if (req.method === "GET") {
    return send(res, 200, { ...existing, stats: customerStats(existing.id) });
  }
  if (req.method === "PATCH") {
    let record;
    try {
      record = customerFrom(await readJson(req), existing);
    } catch (error) {
      return fail(res, 400, "invalid_request", String(error.message ?? error));
    }
    store.customers.set(record.id, record);
    return send(res, 200, record);
  }
  return fail(res, 405, "method_not_allowed", "method not allowed");
}

/**
 * Issuer identities and payout addresses. The emailed code is the same fixed
 * OTP the issuer half of this stub accepts, and the cooldown is not modelled:
 * the specs exercise the flow, not the rate limit.
 */
/** One name per account, however it is cased — the rule the column enforces. */
function nameTaken(world, name, exceptId) {
  const wanted = name.trim().toLowerCase();
  return [...world.issuers.values()].some(
    (row) => row.id !== exceptId && row.name.trim().toLowerCase() === wanted,
  );
}

async function issuerIdentities(req, res, url) {
  const world = issuerWorld(req);
  const shape = (row) => ({
    ...row,
    payout_addresses: (world.issuerAddresses.get(row.id) ?? [])
      .map((id) => world.payoutAddresses.get(id))
      .filter(Boolean),
  });

  if (url.pathname === "/v1/issuers") {
    if (req.method === "POST") {
      const body = await readJson(req);
      const name = String(body.name ?? "").trim();
      const email = String(body.contact_email ?? "")
        .trim()
        .toLowerCase();
      if (!name) return fail(res, 400, "invalid_request", "name is required");
      if (!email.includes("@"))
        return fail(res, 400, "invalid_request", "contact_email is required");
      const now = new Date().toISOString();
      const row = {
        id: randomUUID(),
        name,
        contact_email: email,
        details: body.details ?? null,
        email_verified: false,
        email_verified_at: null,
        created_at: now,
        updated_at: now,
      };
      world.issuers.set(row.id, row);
      return send(res, 201, shape(row));
    }
    if (req.method === "GET") {
      const all = [...world.issuers.values()].sort((a, b) =>
        b.created_at.localeCompare(a.created_at),
      );
      return send(res, 200, { issuers: all.map(shape), next_cursor: null });
    }
    return fail(res, 405, "method_not_allowed", "method not allowed");
  }

  if (url.pathname === "/v1/payout-addresses") {
    if (req.method === "POST") {
      const body = await readJson(req);
      const address = String(body.address ?? "").trim();
      if (!/^0x[0-9a-fA-F]{40}$/.test(address)) {
        return fail(res, 400, "invalid_request", "invalid address");
      }
      const existing = [...world.payoutAddresses.values()].find(
        (row) => row.address.toLowerCase() === address.toLowerCase(),
      );
      const label = String(body.label ?? "").trim();
      if (label && !/^[A-Za-z0-9][A-Za-z0-9 ._'&()-]{0,19}$/.test(label)) {
        return fail(res, 400, "invalid_request", "invalid label");
      }
      if (existing) return send(res, 201, existing);
      const row = {
        id: randomUUID(),
        address,
        label: label || null,
        created_at: new Date().toISOString(),
      };
      world.payoutAddresses.set(row.id, row);
      return send(res, 201, row);
    }
    if (req.method === "GET") {
      return send(res, 200, { payout_addresses: [...world.payoutAddresses.values()] });
    }
    return fail(res, 405, "method_not_allowed", "method not allowed");
  }

  const address = url.pathname.match(/^\/v1\/payout-addresses\/([^/]+)$/);
  if (address) {
    if (req.method !== "DELETE") return fail(res, 405, "method_not_allowed", "method not allowed");
    const id = decodeURIComponent(address[1]);
    if (!world.payoutAddresses.delete(id)) {
      return fail(res, 404, "payout_address_not_found", "No such payout address");
    }
    for (const [issuerId, ids] of world.issuerAddresses) {
      world.issuerAddresses.set(
        issuerId,
        ids.filter((entry) => entry !== id),
      );
    }
    res.writeHead(204, CORS);
    return res.end();
  }

  const match = url.pathname.match(
    /^\/v1\/issuers\/([^/]+)(\/verify\/email\/start|\/verify\/email\/confirm|\/payout-addresses)?$/,
  );
  if (!match) return false;
  const row = world.issuers.get(decodeURIComponent(match[1]));
  if (!row) return fail(res, 404, "issuer_not_found", "No such issuer identity");

  if (match[2] === "/verify/email/start") {
    if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
    if (row.email_verified) {
      return fail(res, 409, "issuer_email_already_verified", "Already verified");
    }
    return send(res, 202, {
      contact_email: row.contact_email,
      resend_available_at: new Date(Date.now() + 60_000).toISOString(),
    });
  }

  if (match[2] === "/verify/email/confirm") {
    if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
    const body = await readJson(req);
    if (String(body.otp ?? "").trim() !== OTP) {
      return fail(res, 401, "otp_invalid", "The code was not accepted");
    }
    row.email_verified = true;
    row.email_verified_at = new Date().toISOString();
    row.updated_at = row.email_verified_at;
    return send(res, 200, shape(row));
  }

  if (match[2] === "/payout-addresses") {
    if (req.method !== "PUT") return fail(res, 405, "method_not_allowed", "method not allowed");
    const body = await readJson(req);
    const ids = Array.isArray(body.payout_address_ids) ? body.payout_address_ids : [];
    for (const id of ids) {
      if (!world.payoutAddresses.has(id)) {
        return fail(res, 404, "payout_address_not_found", "No such payout address");
      }
    }
    world.issuerAddresses.set(row.id, ids);
    return send(res, 200, shape(row));
  }

  if (req.method === "GET") return send(res, 200, shape(row));
  if (req.method === "PATCH") {
    const body = await readJson(req);
    const email = String(body.contact_email ?? "")
      .trim()
      .toLowerCase();
    if (!String(body.name ?? "").trim() || !email.includes("@")) {
      return fail(res, 400, "invalid_request", "name and contact_email are required");
    }
    if (nameTaken(world, String(body.name), row.id)) {
      return fail(res, 409, "issuer_name_taken", "Another identity already uses this name");
    }
    if (email !== row.contact_email) {
      row.email_verified = false;
      row.email_verified_at = null;
    }
    row.name = String(body.name).trim();
    row.contact_email = email;
    row.details = body.details ?? null;
    row.updated_at = new Date().toISOString();
    return send(res, 200, shape(row));
  }
  if (req.method === "DELETE") {
    world.issuers.delete(row.id);
    world.issuerAddresses.delete(row.id);
    res.writeHead(204, CORS);
    return res.end();
  }
  return fail(res, 405, "method_not_allowed", "method not allowed");
}

async function attachments(req, res, url) {
  if (url.pathname === "/v1/attachments") {
    if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
    const body = await readJson(req);
    if (typeof body.filename !== "string" || !body.filename.trim()) {
      return fail(res, 400, "invalid_request", "filename is required");
    }
    const id = randomUUID();
    store.attachments.set(id, {
      id,
      filename: body.filename.trim(),
      bytes: null,
      finalizeCalls: 0,
      status: "pending_upload",
      descriptor: null,
    });
    return send(res, 201, {
      id,
      upload_url: `${ORIGIN}/__upload/${id}`,
      headers: { "content-type": "application/pdf", "x-amz-tagging": "payday-upload=pending" },
      expires_at: new Date(Date.now() + 15 * 60_000).toISOString(),
    });
  }

  const match = url.pathname.match(/^\/v1\/attachments\/([^/]+)\/finalize$/);
  if (!match) return false;
  if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
  const draft = store.attachments.get(decodeURIComponent(match[1]));
  if (!draft) return fail(res, 404, "attachment_not_found", "No such attachment");
  if (draft.status === "ready") return send(res, 200, draft.descriptor);
  if (draft.status === "rejected")
    return fail(res, 422, "attachment_rejected", "Rejected: not a PDF");
  if (!draft.bytes) return fail(res, 422, "attachment_rejected", "Rejected: nothing was uploaded");

  // The scanner has not reported on the first ask, so the client shows its
  // scanning state and comes back with backoff.
  draft.finalizeCalls += 1;
  if (draft.finalizeCalls === 1) {
    return fail(res, 409, "attachment_scan_pending", "Malware scan has not reported yet");
  }
  if (
    !draft.bytes.subarray(0, 5).equals(Buffer.from("%PDF-")) ||
    draft.bytes.length > 5 * 1024 * 1024
  ) {
    draft.status = "rejected";
    return fail(res, 422, "attachment_rejected", "Rejected: not a PDF within limits");
  }
  draft.status = "ready";
  draft.descriptor = {
    id: draft.id,
    filename: draft.filename,
    mime_type: "application/pdf",
    byte_length: String(draft.bytes.length),
    sha256: `0x${createHash("sha256").update(draft.bytes).digest("hex")}`,
  };
  return send(res, 200, draft.descriptor);
}

const MODES = new Set(["permissionless", ...GATED]);

function validateCreate(body) {
  if (!toBaseUnits(body.amount)) return "amount must be a positive USDC amount";
  if (!/^0x[0-9a-fA-F]{40}$/.test(String(body.payout_address ?? "")))
    return "payout_address must be an address";
  if (!body.issuer?.name?.trim()) return "issuer.name is required";
  if (!body.bill_to?.name?.trim()) return "bill_to.name is required";
  const policy = body.payer_policy;
  if (!policy || !MODES.has(policy.mode)) return "payer_policy.mode is invalid";
  if (GATED.has(policy.mode) && !policy.expected_email)
    return `expected_email is required for ${policy.mode}`;
  if (
    policy.mode === "verified_identity" &&
    !(policy.expected_identity?.first_name && policy.expected_identity?.last_name)
  ) {
    return "expected_identity is required for verified_identity";
  }
  if (policy.mode !== "verified_identity" && policy.expected_identity !== undefined) {
    return `expected_identity is not allowed for ${policy.mode}`;
  }
  if (policy.mode === "permissionless" && policy.expected_email !== undefined) {
    return "expected_email is not allowed for permissionless";
  }
  if (body.customer_id !== undefined && !store.customers.has(body.customer_id))
    return "customer_id is not yours";
  if (body.expires_at !== undefined) {
    if (body.expires_in !== undefined) return "choose only one of expires_in or expires_at";
    const at = Date.parse(body.expires_at);
    if (Number.isNaN(at)) return "expires_at must be an RFC 3339 timestamp";
    const lead = at - Date.now();
    if (lead < 10 * 60_000) return "expiry must be at least 10 minutes from now";
    if (lead > 366 * 24 * 3600_000) return "expiry must be no more than 366 days from now";
  }
  return null;
}

async function payments(req, res, url) {
  if (url.pathname === "/v1/payments") {
    if (req.method === "POST") {
      if (!req.headers["idempotency-key"])
        return fail(res, 400, "missing_idempotency_key", "Idempotency-Key is required");
      const body = await readJson(req);
      const problem = validateCreate(body);
      if (problem) return fail(res, 400, "invalid_request", problem);
      let attachment = null;
      if (body.attachment_id !== undefined) {
        const draft = store.attachments.get(body.attachment_id);
        if (!draft) return fail(res, 400, "invalid_request", "attachment_id is not yours");
        if (draft.status !== "ready")
          return fail(res, 409, "attachment_not_ready", "Attachment is not ready");
        draft.status = "attached";
        attachment = draft.descriptor;
      }
      const payment = merchantPayment(body, { attachment });
      store.payments.set(payment.id, payment);
      return send(res, 201, payment);
    }
    if (req.method === "GET") {
      const status = url.searchParams.get("status");
      const customer = url.searchParams.get("customer_id");
      const issuer = url.searchParams.get("issuer_id");
      const verification = url.searchParams.get("verification");
      if (verification && !VERIFICATION_FILTERS.includes(verification)) {
        return fail(res, 400, "invalid_request", "unknown verification filter");
      }
      const verified = (payment) => {
        switch (verification) {
          case "not_required":
            return payment.payer_policy.mode === "permissionless";
          case "pending":
            return (
              payment.payer_policy.mode !== "permissionless" && !payment.verification_completed_at
            );
          case "verified":
            return Boolean(payment.verification_completed_at);
          case "likely_unsolicited":
            return Boolean(payment.likely_unsolicited_at);
          default:
            return true;
        }
      };
      const all = [...store.payments.values()]
        .filter((payment) => !status || payment.status === status)
        .filter((payment) => !customer || payment.customer_id === customer)
        .filter((payment) => !issuer || payment.issuer_id === issuer)
        .filter(verified)
        .sort((a, b) => b.created_at.localeCompare(a.created_at));
      const { page, next } = paginate(all, url.searchParams);
      return send(res, 200, { payments: page.map(summary), next_cursor: next });
    }
    return fail(res, 405, "method_not_allowed", "method not allowed");
  }

  const match = url.pathname.match(
    /^\/v1\/payments\/([^/]+)(\/attachment|\/invoice\.pdf|\/proof|\/transfers|\/verification|\/verification\/review)?$/,
  );
  if (!match) return false;
  const payment = store.payments.get(decodeURIComponent(match[1]));
  if (!payment) return fail(res, 404, "payment_not_found", "No such payment");
  if (match[2] === "/verification/review") {
    if (req.method !== "POST") return fail(res, 405, "method_not_allowed", "method not allowed");
    const attempts = payment.verification_attempts ?? [];
    const latest = [...attempts].reverse().find((attempt) => attempt.kind === "identity");
    if (!latest || !["declined", "review_required"].includes(latest.status)) {
      return fail(res, 409, "review_not_available", "Nothing is declined");
    }
    latest.status = "review_required";
    latest.review ??= {
      requested_at: new Date().toISOString(),
      decision: null,
      reviewer: null,
      note: null,
      decided_at: null,
    };
    return send(res, 200, verificationDetail(payment));
  }
  if (req.method !== "GET") return fail(res, 405, "method_not_allowed", "method not allowed");

  switch (match[2]) {
    case "/verification":
      return send(res, 200, verificationDetail(payment));
    case "/attachment":
      if (!payment.attachment) return fail(res, 404, "attachment_not_found", "No attachment");
      return send(res, 200, {
        ...payment.attachment,
        download_url: `${ORIGIN}/__download/${payment.attachment.id}.pdf`,
      });
    case "/invoice.pdf":
      return sendBytes(res, 200, PDF_BYTES, "application/pdf");
    case "/proof":
      if (payment.status !== "settled")
        return fail(res, 409, "payment_not_settled", "Proof is available once settled");
      return send(res, 200, proofFor(payment));
    case "/transfers":
      return send(res, 200, payment.transfers);
    default:
      return send(res, 200, payment);
  }
}

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");

  try {
    if (req.method === "OPTIONS") {
      res.writeHead(204, CORS);
      return res.end();
    }

    if (url.pathname === "/health") return send(res, 200, { ok: true });
    if (url.pathname === "/__reset") {
      reads.clear();
      return send(res, 200, { ok: true });
    }

    if ((await payer(req, res, url)) !== false) return;
    if (identityProvider(req, res, url) !== false) return;
    if ((await issuer(req, res, url)) !== false) return;
    if ((await objectStore(req, res, url)) !== false) return;

    if (url.pathname.startsWith("/v1/")) {
      if (!authorized(req)) return fail(res, 401, "unauthorized", "Missing or invalid credential");
      if ((await customers(req, res, url)) !== false) return;
      if ((await issuerIdentities(req, res, url)) !== false) return;
      if ((await attachments(req, res, url)) !== false) return;
      if ((await payments(req, res, url)) !== false) return;
    }

    return fail(res, 404, "not_found", "no route");
  } catch (error) {
    if (error instanceof SyntaxError) return fail(res, 400, "invalid_request", "Body is not JSON");
    console.error(error);
    return fail(res, 500, "internal_error", "stub failure");
  }
}).listen(PORT, "127.0.0.1", () => {
  console.log(`stub Payday API on ${ORIGIN}`);
});
