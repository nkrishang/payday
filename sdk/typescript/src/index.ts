export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type PaymentStatus = "awaiting_payment" | "partially_paid" | "paid" | "settled" | "expired" | "returned" | "needs_attention";

export interface Chain { id: string; name: string }
export interface Token { symbol: string; address: string; decimals: number }
export interface AsOf { block: string; at: string }

/** A named party on an invoice. `details` is bounded free text rendered verbatim, never parsed. */
export interface Party {
  name: string;
  email?: string;
  details?: string;
}

export type PayerPolicyMode = "permissionless" | "verified_email" | "merchant_session";

/**
 * Who may pay, and what they must prove first. The verified mode names the
 * expected mailbox. The merchant-session mode names the payer your own
 * application has already signed in, by your own identifier; your server then
 * hands that payer the single-use `client_secret` the create response
 * carries, and no code or vendor is involved. Every assertion is
 * merchant-supplied and immutable once the invoice is issued.
 */
export type PayerPolicy =
  | { mode: "permissionless" }
  | { mode: "verified_email"; expected_email: string }
  | { mode: "merchant_session"; payer_reference: string };

export interface AttachmentDescriptor {
  id: string;
  filename: string;
  mime_type: "application/pdf";
  /** Decimal string, like every exact integer on the API. */
  byte_length: string;
  /** 0x-prefixed lowercase hex of the stored bytes; committed into the payment address. */
  sha256: string;
  /** Short-lived signed URL, present only on attachment reads. */
  download_url?: string;
}

export interface CreatePayment {
  amount: string;
  payout_address: string;
  issuer: Party;
  bill_to: Party;
  payer_policy: PayerPolicy;
  customer_id?: string;
  /**
   * The saved issuer identity this is issued under. `issuer` above is still
   * the snapshot the document carries and what the address commits to; this
   * only records which identity it came from, and keeps pointing at it after
   * that identity is renamed or moved to another mailbox.
   */
  issuer_id?: string;
  notes?: string;
  heading?: string;
  reference?: string;
  metadata?: Record<string, JsonValue>;
  /** A finalized attachment from `attachments.upload`; one PDF per invoice. */
  attachment_id?: string;
  expires_in?: number;
  expires_at?: string;
  chain_id?: string;
  token_address?: string;
}

/** The commitment that ties the issued document to the payment address. */
export interface Attribution { version: number; hash: string }

export interface Payment {
  id: string;
  payment_url: string;
  status: PaymentStatus;
  chain: Chain;
  currency: string;
  token: Token;
  /**
   * The one-time payment address. Null until the payer attests the wallet
   * they will pay from: the address commits to that attestation, so it
   * cannot exist before it.
   */
  address: string | null;
  address_explorer_url: string | null;
  payout_address: string;
  /**
   * The wallet the payer attested, once they have. Only transfers from it
   * are the payer's; overpayment remainders, expired balances, and late
   * transfers return to it.
   */
  payer_wallet: string | null;
  /** Always equal to `payer_wallet`: the address's recovery term. */
  recovery_address: string | null;
  /** When the attestation was accepted and the address derived. */
  wallet_bound_at: string | null;
  amount: string;
  amount_base_units: string;
  received: string;
  received_base_units: string;
  remaining: string;
  remaining_base_units: string;
  fee_amount: string;
  fee_amount_base_units: string;
  net_amount: string;
  net_amount_base_units: string;
  issuer: Party;
  bill_to: Party;
  notes: string | null;
  heading: string | null;
  reference: string | null;
  customer_id: string | null;
  /** The issuer identity it was issued under; immutable once issued. */
  issuer_id: string | null;
  metadata: Record<string, JsonValue>;
  /** Full policy including assertions; merchant-only, never on the payer route. */
  payer_policy: PayerPolicy;
  attachment: AttachmentDescriptor | null;
  /**
   * `merchant_session` only, and only on the `201` that issued the payment:
   * the first single-use client secret, valid for fifteen minutes. Absent on
   * every later read and on idempotent replays; the API stores only its hash.
   * Send the payer to `checkoutUrl(payment, client_secret)`; mint another with
   * `payments.createClientSecret` when they come back.
   */
  client_secret?: string;
  client_secret_expires_at?: string;
  verification_completed_at: string | null;
  /** Set when finalized funds first arrived from a wallet other than the attested one. */
  likely_unsolicited_at: string | null;
  attribution: Attribution;
  created_at: string;
  updated_at: string;
  expires_at: string;
  expires_in?: number;
  paid_at: string | null;
  paid_at_block: string | null;
  settled_at: string | null;
  settled_block: string | null;
  expired_at: string | null;
  cancellation_requested_at: string | null;
  settlement_tx_hash: string | null;
  settlement_explorer_url: string | null;
  as_of: AsOf | null;
  /** What a third party needs to execute the contract; null until the address exists. */
  self_settlement: { factory: string; salt: string } | null;
  attention: { code: string; message: string; action: string } | null;
  transfers: Transfer[];
  indexer_freshness: { last_indexed_block: string | null; last_finalized_block: string | null; cursor_updated_at: string | null };
}

export type VerificationFactStatus = "not_required" | "pending" | "approved" | "declined";

/**
 * Each fact the policy needs, on its own. `email` is the identity policy and
 * `complete` is whether the session satisfies it (which unlocks the content).
 * `wallet` is never `not_required`: every request needs the payer's wallet
 * attestation before it has an address, and it belongs to the request, not
 * the session.
 */
export interface VerificationRequirements {
  email: VerificationFactStatus;
  wallet: VerificationFactStatus;
  /** Your application opened the checkout by exchanging a client secret. */
  merchant_session: VerificationFactStatus;
  complete: boolean;
}

/** A fresh single-use client secret for a `merchant_session` payment. Returned once; stored hashed. */
export interface ClientSecret { client_secret: string; expires_at: string }

/** A payer session minted by exchanging a client secret on the hosted checkout. */
export interface ExchangeClientSecret {
  payer_session: string;
  expires_at: string;
  requirements: VerificationRequirements;
}

/** The fragment key the hosted checkout reads a client secret from. */
export const CLIENT_SECRET_FRAGMENT_KEY = "cs";

/**
 * The URL to send an authenticated payer to for a `merchant_session` payment:
 * the payment page with the client secret in the fragment, which never reaches
 * a server log, a Referer header, or an analytics beacon. Redirect to it or
 * open it in a frame; never write it to a log.
 */
export function checkoutUrl(payment: Pick<Payment, "payment_url">, clientSecret: string): string {
  if (!clientSecret) throw new TypeError("clientSecret is required");
  return `${payment.payment_url}#${CLIENT_SECRET_FRAGMENT_KEY}=${encodeURIComponent(clientSecret)}`;
}

/** The policy as the payer may see it: the mode and a masked mailbox hint such as `a****@e***.com`. */
export interface PayerPolicySummary { mode: PayerPolicyMode; expected_email_hint: string | null }

/** A payer session minted by `verification.startEmail`; the token is opaque and stored hashed by the API. */
export interface StartEmailVerification { payer_session: string; expires_at: string }

/** What a session (or, without one, the invoice) has established. */
export interface VerificationStatus {
  requirements: VerificationRequirements;
}

/**
 * One verification attempt as the merchant sees it: what was attempted and
 * where it stands. Never the code or the payer's session.
 */
export interface VerificationAttempt {
  id: string;
  /** A `merchant_session` attempt is the exchange itself, recorded approved. */
  kind: "email" | "wallet" | "merchant_session";
  status: "pending" | "approved" | "abandoned";
  verified_at: string | null;
  created_at: string;
}

/** The merchant's verification view of one invoice: each fact on its own and every attempt. */
export interface VerificationDetail {
  payer_policy_mode: PayerPolicyMode;
  verification_completed_at: string | null;
  likely_unsolicited_at: string | null;
  facts: VerificationRequirements;
  attempts: VerificationAttempt[];
}

/** Invoice content that a gated invoice withholds until verification completes. */
export interface PayerInvoiceDetails {
  amount: string;
  amount_base_units: string;
  bill_to: Party;
  notes: string | null;
  reference: string | null;
  attachment: AttachmentDescriptor | null;
}

/**
 * The narrowed projection served to anyone holding a payment link.
 *
 * Deliberately carries no merchant data: no payout or recovery address, no
 * metadata, customer, or policy assertions. The payment page is world-readable,
 * so this is the only payment shape safe to render on it.
 *
 * Gated invoices disclose progressively. Until `content_unlocked` is true the
 * mechanics (`chain`, `token`, amounts, `address`, `payment_uri`) and `invoice`
 * are null; only the issuer name, heading, status, and requirement state show.
 */
export interface PayerPayment {
  id: string;
  issuer_name: string;
  heading: string | null;
  payer_policy: PayerPolicySummary;
  requirements: VerificationRequirements;
  status: PaymentStatus;
  /** Whether the gateway still considers this address payable. */
  payable: boolean;
  expires_at: string;
  /** Gateway wall-clock unix seconds, so a countdown never trusts the payer's device. */
  server_timestamp: string;
  settlement_tx_hash: string | null;
  settlement_explorer_url: string | null;
  /** Safety guidance shown only when payout needs operator attention. */
  payer_message: string | null;
  content_unlocked: boolean;
  chain: Chain | null;
  token: Token | null;
  amount: string | null;
  amount_base_units: string | null;
  received: string | null;
  received_base_units: string | null;
  remaining: string | null;
  remaining_base_units: string | null;
  /** The wallet bound to this request, once a payer has attested one. */
  payer_wallet: string | null;
  /** Present once unlocked and a wallet is bound; null before either. */
  address: string | null;
  address_explorer_url: string | null;
  /** EIP-681 request for the amount still due; null while locked, unbound, or once not payable. */
  payment_uri: string | null;
  invoice: PayerInvoiceDetails | null;
}

/** EIP-712 typed data exactly as `eth_signTypedData_v4` / viem's `signTypedData` take it. */
export interface PayerAttestationTypedData {
  domain: { name: string; version: string; chainId: number; verifyingContract: string };
  primaryType: "PayerAttestation";
  types: {
    EIP712Domain: Array<{ name: string; type: string }>;
    PayerAttestation: Array<{ name: string; type: string }>;
  };
  message: {
    statement: string;
    attributionHash: string;
    wallet: string;
    nonce: string;
    expiresAt: number;
  };
}

/** A wallet challenge: the session it belongs to and the document to sign. */
export interface WalletChallenge {
  payer_session: string;
  expires_at: string;
  typed_data: PayerAttestationTypedData;
}

export interface PaymentSummary {
  id: string;
  heading: string | null;
  bill_to_name: string;
  reference: string | null;
  metadata: Record<string, JsonValue>;
  payer_policy_mode: PayerPolicyMode;
  customer_id: string | null;
  issuer_id: string | null;
  has_attachment: boolean;
  verification_completed_at: string | null;
  likely_unsolicited_at: string | null;
  created_at: string;
  status: PaymentStatus;
  amount: string;
  received: string;
  cancellation_requested_at: string | null;
}
/** Verification is a separate fact from the payment's status, so it filters separately. */
export type VerificationFilter = "not_required" | "pending" | "verified" | "likely_unsolicited";
export interface ListPaymentsParams {
  starting_after?: string;
  status?: PaymentStatus;
  reference?: string;
  /** Only requests billed to this customer. */
  customer_id?: string;
  /** Only requests issued under this identity. */
  issuer_id?: string;
  verification?: VerificationFilter;
  limit?: number;
}
export interface PaymentPage { payments: PaymentSummary[]; next_cursor: string | null }
export interface Transfer {
  transaction_hash: string; explorer_url: string | null; sender: string; amount: string; amount_base_units: string;
  block: string; timestamp: string; disposition: "credited" | "late" | "zero"; collected: boolean;
}
export interface CancelPaymentResponse { payment: Payment; advisory: string }
/** Internal: the dashboard onboarding walkthrough's one real demo transfer. */
export interface OnboardingPaymentResponse { payer_session: string; tx_hash: string }

/** A saved payout wallet, EIP-55 checksummed and unique per account. */
export interface PayoutAddress {
  id: string;
  address: string;
  label: string | null;
  created_at: string;
}

/**
 * A saved issuer identity: the party an invoice is issued under, its contact
 * mailbox, and the wallets it may settle to. Issuance is unchanged — a payment
 * still carries its own `issuer` and `payout_address` snapshot — so editing an
 * identity never touches an invoice already issued.
 */
export interface Issuer {
  id: string;
  name: string;
  contact_email: string;
  details: string | null;
  /** Whether the contact mailbox was proven with an emailed code. */
  email_verified: boolean;
  email_verified_at: string | null;
  /** In association order; the first is the sensible default. */
  payout_addresses: PayoutAddress[];
  created_at: string;
  updated_at: string;
}

export interface CreateIssuer {
  name: string;
  contact_email: string;
  details?: string;
}
export interface ListIssuersParams { starting_after?: string; limit?: number }
export interface IssuerPage { issuers: Issuer[]; next_cursor: string | null }
/** Where the code went, and when another may be asked for. */
export interface StartIssuerEmailVerification {
  contact_email: string;
  resend_available_at: string;
}
export interface CreatePayoutAddress { address: string; label?: string }
export interface PayoutAddressList { payout_addresses: PayoutAddress[] }

export interface Customer {
  id: string;
  name: string;
  email: string | null;
  details: string | null;
  created_at: string;
  updated_at: string;
}
export interface CreateCustomer { name: string; email?: string; details?: string }
/** Full replacement of the editable fields; a field left out or set to null is cleared. */
export interface UpdateCustomer { name: string; email?: string | null; details?: string | null }
export interface ListCustomersParams { starting_after?: string; limit?: number }
export interface CustomerPage { customers: Customer[]; next_cursor: string | null }
/** Base units, like a payment's own `amount_base_units` — scale for display. */
export interface CustomerStats {
  request_count: number;
  collected_base_units: string;
  pending_base_units: string;
}
/** Only `customers.get` carries stats; a list of many would mean one aggregate query per row. */
export interface CustomerDetail extends Customer {
  stats: CustomerStats;
}

/** A presigned slot to PUT one PDF into; `headers` must be sent verbatim with the PUT. */
export interface AttachmentUpload {
  id: string;
  upload_url: string;
  headers: Record<string, string>;
  expires_at: string;
}

export interface AttachmentCommitment { id: string; byte_length: string; sha256: string }

/**
 * The exact document hashed at issuance; every string is canonical (decimal
 * amounts, EIP-55 addresses). It carries no recovery address: that is the
 * payer's attested wallet, which enters the payment address through the
 * attestation rather than through this document.
 */
export interface CanonicalIssuanceSnapshot {
  schema: "payday.invoice";
  canonicalization: "RFC8785";
  issuer: Party;
  bill_to: Party;
  amount_base_units: string;
  notes: string | null;
  heading: string | null;
  reference: string | null;
  expiration_timestamp: string;
  payer_policy: PayerPolicy;
  attachment: AttachmentCommitment | null;
  chain_id: string;
  token_address: string;
  receiver_address: string;
  factory_address: string;
}

/**
 * The payer's wallet attestation as the proof carries it: the exact typed
 * data the wallet signed, its EIP-712 digest, and the signature. The proof's
 * salt is `keccak256("PAYDAY_SALT_V2" || attribution_hash || digest)` and the
 * wallet is the address's recovery term.
 */
export interface PayerWalletAttestation {
  address: string;
  typed_data: PayerAttestationTypedData;
  digest: string;
  signature: string;
  method: "ecdsa";
}

/** One fact Payday observed: the proven mailbox or the accepted wallet signature. */
export interface VerificationFact {
  kind: "mailbox" | "merchant_session" | "wallet";
  provider: "auth0" | "merchant" | "payday";
  at: string;
}

export interface ProofTransfer {
  transaction_hash: string;
  /** Decimal receipt log index; canonical event identity with transaction_hash. */
  log_index: string;
  sender: string;
  recipient: string;
  amount_base_units: string;
  block_number: string;
}

/**
 * What Payday signs about a verification outcome. The commitment (attribution
 * hash, chain, CREATE3 address, payer wallet, and the nonce inside the wallet's
 * attestation) is in the signed payload so the attestation vouches for this
 * request and this payer only, not for any proof reusing its id.
 */
export interface VerificationAttestationPayload {
  version: string;
  payment_id: string;
  /** `0x` hex, 32 bytes: the attribution hash of the canonical invoice. */
  attribution_hash: string;
  /** Decimal, as in the canonical issuance snapshot. */
  chain_id: string;
  /** EIP-55 checksummed CREATE3 payment address. */
  payment_address: string;
  payer_wallet: string;
  wallet_nonce: string;
  payer_policy_mode: PayerPolicyMode;
  result: string;
  verified_at: string | null;
  wallet_bound_at: string;
  facts: VerificationFact[];
}

/** Payday-attested, not address-committed: verification happens after issuance. */
export interface SignedVerificationAttestation {
  payload: VerificationAttestationPayload;
  signer: string;
  signature: string;
}

/**
 * Offline-verifiable record tying the issued invoice to the wallet its payer
 * attested, to the payment address both commit to, to the transfers from that
 * wallet that paid it, and to the transaction that settled it. It is checked
 * without Payday: `gateway_core::verify_proof` holds the offline checks.
 */
export interface ProofOfPayment {
  version: string;
  payment_id: string;
  canonical_issuance_snapshot: CanonicalIssuanceSnapshot;
  canonicalization: string;
  attribution_hash: string;
  payer_wallet: PayerWalletAttestation;
  salt: string;
  chain_id: string;
  factory_address: string;
  payment_address: string;
  token_address: string;
  /** The payer's attested wallet. */
  recovery_address: string;
  /**
   * The fulfilment transaction (the payment's `settlement_tx_hash`) that
   * forwarded the funds to the receiver; not one of `transfers`, which are the
   * payer's USDC transfers into the payment address.
   */
  settlement_transaction_hash: string;
  transfers: ProofTransfer[];
  verification: SignedVerificationAttestation;
}

export interface ServiceStatus {
  chain: Chain & { finalized_block: string | null; finalized_at: string | null };
  indexer: { cursor_block: string | null; cursor_at: string | null; lag_blocks: number | null };
  sweeper: { state: string; queued: number };
}
export interface Webhook { id: string; url: string; created_at: string; secret?: string }
export interface TestDelivery { delivery_id: string }
export interface RemovedWebhook { id: string; disabled: true }
export interface DeliveryAttempt {
  number: number; attempted_at: string; duration_ms: number; status: number | null; error: string | null;
}
export interface WebhookDelivery {
  id: string; event_id: string; endpoint_id: string; state: string; attempt_count: number;
  next_attempt_at: string; delivered_at: string | null; attempts: DeliveryAttempt[];
}

/** The account: who it is, where it settles, and its key state — never the key itself, which only `account.issueApiKey` ever returns. */
export interface AccountMetadata {
  account_id: string;
  /** The mailbox the account signs in with. */
  email: string | null;
  /**
   * The account's own EVM wallet (EIP-55), where deposits settle by default.
   * `null` only until the first dashboard session that carried one.
   */
  wallet_address: string | null;
  /** e.g. `"…aB3f9Q"` — the key's last few characters, or `null` with no active key. */
  key_hint: string | null;
  generation: number;
  created_at: string;
  rotated_at: string | null;
  /** Set for 24 hours after a rotation, while the replaced key still authenticates. */
  previous_key_expires_at: string | null;
  revoked_at: string | null;
}
export interface IssuedApiKey {
  /** Shown once. Nothing later, including `account.get`, can return it again. */
  api_key: string;
  generation: number;
  replaced_previous_key: boolean;
}

interface PaydayClientCommonOptions {
  baseUrl?: string;
  fetch?: typeof globalThis.fetch;
}

/**
 * Exactly one credential: a server-side API key, or the session token a
 * signed-in dashboard holds (its Privy identity token). Both travel as the
 * same bearer header; the API tells them apart.
 */
export type PaydayClientOptions = PaydayClientCommonOptions &
  ({ apiKey: string; accessToken?: undefined } | { accessToken: string; apiKey?: undefined });

export interface PaydayPayerClientOptions {
  baseUrl?: string;
  fetch?: typeof globalThis.fetch;
}

export interface UploadOptions {
  signal?: AbortSignal;
  /** Upper bound in milliseconds on waiting for the malware scan; default two minutes. */
  scanTimeout?: number;
}

export class PaydayError extends Error {
  readonly code: string;
  readonly requestId: string | undefined;
  readonly status: number;
  /** Short-lived proof returned when an accepted OTP could not be persisted. */
  readonly continuation: string | undefined;

  constructor(message: string, code: string, status: number, requestId?: string, continuation?: string) {
    super(message);
    this.name = "PaydayError";
    this.code = code;
    this.status = status;
    this.requestId = requestId;
    this.continuation = continuation;
  }
}

const DEFAULT_BASE_URL = "https://api.payday.sh";
const DEFAULT_SCAN_TIMEOUT_MS = 120_000;
const SCAN_POLL_INITIAL_MS = 1_000;
const SCAN_POLL_MAX_MS = 8_000;
const PAYER_SESSION_HEADER = "Payday-Payer-Session";

interface RequestOptions {
  method?: string;
  body?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
  accept?: string;
}

/**
 * Every Payday response is `Cache-Control: no-store`, so requests opt out of
 * caching explicitly. This also stops frameworks that patch `fetch` with a
 * caching default (Next.js) from serving a stale payment.
 */
async function send(
  fetcher: typeof globalThis.fetch,
  baseUrl: string,
  path: string,
  options: RequestOptions = {},
): Promise<Response> {
  const headers: Record<string, string> = { Accept: options.accept ?? "application/json", ...options.headers };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetcher(`${baseUrl}${path}`, {
    method: options.method ?? "GET",
    headers,
    cache: "no-store",
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
  });
  if (!response.ok) throw await errorFrom(response);
  return response;
}

async function errorFrom(response: Response): Promise<PaydayError> {
  const text = await response.text();
  let wire: { error?: { code?: string; message?: string }; request_id?: string; continuation?: string } | undefined;
  try { wire = text ? JSON.parse(text) : undefined; } catch { wire = undefined; }
  return new PaydayError(
    wire?.error?.message ?? response.statusText ?? "Payday API request failed",
    wire?.error?.code ?? "http_error", response.status,
    wire?.request_id ?? response.headers.get("x-request-id") ?? undefined,
    wire?.continuation,
  );
}

async function request<T>(
  fetcher: typeof globalThis.fetch,
  baseUrl: string,
  path: string,
  options: RequestOptions = {},
): Promise<T> {
  const response = await send(fetcher, baseUrl, path, options);
  const text = await response.text();
  let data: unknown;
  try { data = text ? JSON.parse(text) : undefined; } catch { data = undefined; }
  return data as T;
}

function normalizeBaseUrl(baseUrl: string | undefined): string {
  return (baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
}

function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(signal.reason); return; }
    const timer = setTimeout(() => { signal?.removeEventListener("abort", onAbort); resolve(); }, ms);
    function onAbort() { clearTimeout(timer); reject(signal?.reason); }
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

export class PaydayClient {
  private readonly credential: string;
  private readonly baseUrl: string;
  private readonly fetcher: typeof globalThis.fetch;

  constructor(options: PaydayClientOptions) {
    const { apiKey, accessToken } = options;
    if (Boolean(apiKey) === Boolean(accessToken)) {
      throw new TypeError("exactly one of apiKey or accessToken is required");
    }
    this.credential = (apiKey ?? accessToken) as string;
    this.baseUrl = normalizeBaseUrl(options.baseUrl);
    this.fetcher = options.fetch ?? globalThis.fetch;
    if (!this.fetcher) throw new TypeError("fetch is required");
  }

  readonly payments = {
    create: (payment: CreatePayment, idempotencyKey: string): Promise<Payment> => {
      if (!idempotencyKey) throw new TypeError("idempotencyKey is required");
      return this.request("/v1/payments", { method: "POST", body: payment, idempotencyKey });
    },
    get: (id: string, options: { waitForChange?: boolean; timeout?: number } = {}): Promise<Payment> => {
      const params = options.waitForChange ? { wait_for: "change", timeout: options.timeout } : {};
      return this.request(`/v1/payments/${encodeURIComponent(id)}${query(params)}`);
    },
    list: (params: ListPaymentsParams = {}): Promise<PaymentPage> =>
      this.request(`/v1/payments${query(params)}`),
    cancel: (id: string): Promise<CancelPaymentResponse> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/cancel`, { method: "POST" }),
    /**
     * Internal: the dashboard onboarding walkthrough's one real demo
     * transfer and verification. Refuses `409 onboarding_payment_not_eligible`
     * for anything not addressed to Payday's own onboarding mailbox — not a
     * general merchant feature.
     */
    onboardingPayment: (id: string): Promise<OnboardingPaymentResponse> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/onboarding-payment`, { method: "POST" }),
    transfers: (id: string): Promise<Transfer[]> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/transfers`),
    /** The invoice's PDF attachment with a short-lived `download_url`. */
    attachment: (id: string): Promise<AttachmentDescriptor> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/attachment`),
    /** Payday's deterministic invoice summary; the same invoice always renders byte-identical bytes. */
    invoicePdf: async (id: string): Promise<Blob> => {
      const response = await this.send(`/v1/payments/${encodeURIComponent(id)}/invoice.pdf`, { accept: "application/pdf" });
      return response.blob();
    },
    /** Available once the payment is settled; `409 payment_not_settled` before. */
    proof: (id: string): Promise<ProofOfPayment> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/proof`),
    /** Every verification attempt on the invoice, each fact reported separately. */
    verification: (id: string): Promise<VerificationDetail> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/verification`),
    /**
     * A fresh single-use client secret for a `merchant_session` payment, for a
     * payer your application signs in again after the first secret was spent
     * or expired. Earlier unspent secrets stay valid until they expire.
     * `409 verification_not_required` for a permissionless payment,
     * `409 verification_method_not_applicable` for a `verified_email` one, and
     * `410 payment_not_payable` once the payment is closed without having verified.
     */
    createClientSecret: (id: string): Promise<ClientSecret> =>
      this.request(`/v1/payments/${encodeURIComponent(id)}/client-secret`, { method: "POST" }),
  };

  readonly customers = {
    create: (customer: CreateCustomer): Promise<Customer> =>
      this.request("/v1/customers", { method: "POST", body: customer }),
    get: (id: string): Promise<CustomerDetail> =>
      this.request(`/v1/customers/${encodeURIComponent(id)}`),
    list: (params: ListCustomersParams = {}): Promise<CustomerPage> =>
      this.request(`/v1/customers${query(params)}`),
    update: (id: string, customer: UpdateCustomer): Promise<Customer> =>
      this.request(`/v1/customers/${encodeURIComponent(id)}`, { method: "PATCH", body: customer }),
  };

  /**
   * Saved issuer identities. An identity is a convenience for whoever issues:
   * `payments.create` still takes the party and the payout address inline and
   * snapshots them, so nothing here can change an invoice already issued. The
   * contact mailbox is the exception worth proving — payers are told to write
   * to it — and it is proven with the same emailed code the dashboard signs in
   * with.
   */
  readonly issuers = {
    create: (issuer: CreateIssuer): Promise<Issuer> =>
      this.request("/v1/issuers", { method: "POST", body: issuer }),
    get: (id: string): Promise<Issuer> =>
      this.request(`/v1/issuers/${encodeURIComponent(id)}`),
    list: (params: ListIssuersParams = {}): Promise<IssuerPage> =>
      this.request(`/v1/issuers${query(params)}`),
    /** Full replacement; a different `contact_email` clears the verification. */
    update: (id: string, issuer: CreateIssuer): Promise<Issuer> =>
      this.request(`/v1/issuers/${encodeURIComponent(id)}`, { method: "PATCH", body: issuer }),
    remove: (id: string): Promise<void> =>
      this.request(`/v1/issuers/${encodeURIComponent(id)}`, { method: "DELETE" }),
    /**
     * Emails a code to the identity's stored contact address — the request
     * never names a mailbox. One code per identity per minute
     * (`otp_resend_cooldown`).
     */
    startEmailVerification: (id: string): Promise<StartIssuerEmailVerification> =>
      this.request(`/v1/issuers/${encodeURIComponent(id)}/verify/email/start`, { method: "POST" }),
    confirmEmailVerification: (id: string, otp: string): Promise<Issuer> =>
      this.request(`/v1/issuers/${encodeURIComponent(id)}/verify/email/confirm`, {
        method: "POST",
        body: { otp },
      }),
    /** Replaces the whole set of addresses this identity may settle to. */
    setPayoutAddresses: (id: string, payoutAddressIds: string[]): Promise<Issuer> =>
      this.request(`/v1/issuers/${encodeURIComponent(id)}/payout-addresses`, {
        method: "PUT",
        body: { payout_address_ids: payoutAddressIds },
      }),
  };

  readonly payoutAddresses = {
    /** Saving a wallet you already saved returns the row you have. */
    create: (input: CreatePayoutAddress): Promise<PayoutAddress> =>
      this.request("/v1/payout-addresses", { method: "POST", body: input }),
    list: (): Promise<PayoutAddressList> => this.request("/v1/payout-addresses"),
    remove: (id: string): Promise<void> =>
      this.request(`/v1/payout-addresses/${encodeURIComponent(id)}`, { method: "DELETE" }),
  };

  readonly attachments = {
    create: (input: { filename: string }, options: { signal?: AbortSignal } = {}): Promise<AttachmentUpload> =>
      this.request("/v1/attachments", { method: "POST", body: input, ...options }),
    /**
     * Asks the API to admit the uploaded bytes. Throws `attachment_scan_pending`
     * (409) while the malware scan is still running and `attachment_rejected`
     * (422) when the object is not a clean PDF within limits.
     */
    finalize: (id: string, options: { signal?: AbortSignal } = {}): Promise<AttachmentDescriptor> =>
      this.request(`/v1/attachments/${encodeURIComponent(id)}/finalize`, { method: "POST", ...options }),
    /**
     * The whole exchange: reserve a slot, PUT the bytes with the presigned
     * headers, then poll `finalize` with backoff until the scan admits or
     * rejects the file. Resolves to the descriptor to pass as `attachment_id`.
     */
    upload: async (file: Blob | Uint8Array, filename: string, options: UploadOptions = {}): Promise<AttachmentDescriptor> => {
      const { signal } = options;
      const slot = await this.attachments.create({ filename }, { ...(signal === undefined ? {} : { signal }) });
      const body = file instanceof Uint8Array ? new Blob([file as BlobPart]) : file;
      const put = await this.fetcher(slot.upload_url, {
        method: "PUT",
        headers: slot.headers,
        body,
        ...(signal === undefined ? {} : { signal }),
      });
      if (!put.ok) {
        throw new PaydayError(`Attachment upload failed with HTTP ${put.status}`, "attachment_upload_failed", put.status);
      }
      const deadline = Date.now() + (options.scanTimeout ?? DEFAULT_SCAN_TIMEOUT_MS);
      let delay = SCAN_POLL_INITIAL_MS;
      for (;;) {
        try {
          return await this.attachments.finalize(slot.id, { ...(signal === undefined ? {} : { signal }) });
        } catch (error) {
          if (!(error instanceof PaydayError) || error.code !== "attachment_scan_pending") throw error;
          if (Date.now() + delay > deadline) {
            throw new PaydayError(
              `Attachment ${slot.id} was not scanned in time; call attachments.finalize later`,
              "attachment_scan_timeout", error.status, error.requestId,
            );
          }
          await sleep(delay, signal);
          delay = Math.min(delay * 2, SCAN_POLL_MAX_MS);
        }
      }
    },
  };

  readonly webhooks = {
    add: (url: string): Promise<Webhook> => this.request("/v1/webhooks", { method: "POST", body: { url } }),
    list: (): Promise<Webhook[]> => this.request("/v1/webhooks"),
    remove: (id: string): Promise<RemovedWebhook> =>
      this.request(`/v1/webhooks/${encodeURIComponent(id)}`, { method: "DELETE" }),
    test: (id: string): Promise<TestDelivery> =>
      this.request(`/v1/webhooks/${encodeURIComponent(id)}/test`, { method: "POST" }),
    deliveries: (): Promise<WebhookDelivery[]> => this.request("/v1/webhook-deliveries"),
  };

  status(): Promise<ServiceStatus> { return this.request("/v1/status"); }

  /**
   * The signed-in account: its mailbox, its wallet, and its key metadata —
   * never the raw key. `get` works with either credential. The two mutations
   * work only with a dashboard session: whoever holds an API key must not be
   * able to mint another from it, so a client built with `apiKey` gets
   * `identity_unauthorized` (401) from them. Pass `expectedGeneration` from
   * the account's current `generation`; a mismatch throws `PaydayError` with
   * code `api_key_generation_conflict`, meaning something else changed the
   * key first — re-read the account and, if the merchant still wants to
   * proceed, retry with the generation that came back.
   */
  readonly account = {
    get: (): Promise<AccountMetadata> => this.request("/v1/account"),
    /** Issues a first key, or rotates the current one; the previous key, if any, keeps working for 24 hours. */
    issueApiKey: (expectedGeneration: number): Promise<IssuedApiKey> =>
      this.request("/v1/account/api-key", {
        method: "POST",
        body: { expected_generation: expectedGeneration },
      }),
    /** Immediately invalidates the current key and any key still in its rotation grace window. */
    revokeApiKey: (expectedGeneration: number): Promise<void> =>
      this.request("/v1/account/api-key", {
        method: "DELETE",
        body: { expected_generation: expectedGeneration },
      }),
  };

  private request<T>(path: string, options: { method?: string; body?: unknown; idempotencyKey?: string; signal?: AbortSignal } = {}): Promise<T> {
    return request<T>(this.fetcher, this.baseUrl, path, this.authorized(options));
  }

  private send(path: string, options: { method?: string; signal?: AbortSignal; accept?: string } = {}): Promise<Response> {
    return send(this.fetcher, this.baseUrl, path, this.authorized(options));
  }

  private authorized(options: { method?: string; body?: unknown; idempotencyKey?: string; signal?: AbortSignal; accept?: string }): RequestOptions {
    const headers: Record<string, string> = { Authorization: `Bearer ${this.credential}` };
    if (options.idempotencyKey !== undefined) headers["Idempotency-Key"] = options.idempotencyKey;
    return {
      headers,
      ...(options.method === undefined ? {} : { method: options.method }),
      ...(options.body === undefined ? {} : { body: options.body }),
      ...(options.signal === undefined ? {} : { signal: options.signal }),
      ...(options.accept === undefined ? {} : { accept: options.accept }),
    };
  }
}

/**
 * Keyless client for the public payer routes behind a `payment_url`.
 *
 * A payment link is intentionally open: anyone holding it may view the payment
 * and fulfil it. These routes accept no API key, so never pass a secret here.
 * A payer session token, obtained by completing verification on a gated
 * invoice, travels in `Payday-Payer-Session` and unlocks the withheld content.
 */
export class PaydayPayerClient {
  private readonly baseUrl: string;
  private readonly fetcher: typeof globalThis.fetch;

  constructor(options: PaydayPayerClientOptions = {}) {
    this.baseUrl = normalizeBaseUrl(options.baseUrl);
    this.fetcher = options.fetch ?? globalThis.fetch;
    if (!this.fetcher) throw new TypeError("fetch is required");
  }

  readonly payments = {
    get: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<PayerPayment> =>
      request<PayerPayment>(this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}`, payerOptions(options)),

    /**
     * The invoice's PDF with a short-lived `download_url`. Answers
     * `401 verification_required` while the content is still gated.
     */
    attachment: (id: string, payerSession?: string, options: { signal?: AbortSignal } = {}): Promise<AttachmentDescriptor> =>
      request<AttachmentDescriptor>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/attachment`,
        payerOptions({ ...options, ...(payerSession === undefined ? {} : { payerSession }) }),
      ),

    /**
     * The payment's QR: an SVG of the same EIP-681 request the page shows, for
     * the amount still due. Render it from an object URL. It answers
     * `401 verification_required` while the content is gated and
     * `410 payment_not_payable` the moment the address must stop being shown.
     * The session travels in a header, never in the URL.
     */
    qr: async (id: string, payerSession?: string, options: { signal?: AbortSignal } = {}): Promise<Blob> => {
      const response = await send(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/qr`,
        payerOptions({ ...options, ...(payerSession === undefined ? {} : { payerSession }), accept: "image/svg+xml" }),
      );
      return response.blob();
    },
  };

  /**
   * Email verification for a gated invoice. The code goes to the mailbox the
   * merchant asserted at issuance; the payer only ever types the code. These
   * writes answer cross-origin requests from the hosted checkout only.
   */
  readonly verification = {
    /** Send (or, with a session, resend) the code and get the session it belongs to. */
    startEmail: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<StartEmailVerification> =>
      request<StartEmailVerification>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/verify/email/start`,
        { method: "POST", ...payerOptions(options) },
      ),
    /** Exchange the code; answers `401 otp_invalid` for a wrong one. */
    confirmEmail: (id: string, otp: string, payerSession: string, options: { signal?: AbortSignal } = {}): Promise<VerificationStatus> =>
      request<VerificationStatus>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/verify/email/confirm`,
        { method: "POST", body: { otp }, ...payerOptions({ ...options, payerSession }) },
      ),
    /** Resume persistence after `verification_persistence_unavailable`, without reusing the spent OTP. */
    continueEmail: (id: string, continuation: string, payerSession: string, options: { signal?: AbortSignal } = {}): Promise<VerificationStatus> =>
      request<VerificationStatus>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/verify/email/confirm`,
        { method: "POST", body: { continuation }, ...payerOptions({ ...options, payerSession }) },
      ),
    /** The session's facts, or the invoice's without a session. */
    status: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<VerificationStatus> =>
      request<VerificationStatus>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/verify`,
        payerOptions(options),
      ),
    /**
     * Exchange a `merchant_session` client secret for the payer session it
     * opens. Exactly once: a second exchange answers `409 client_secret_used`;
     * an unknown, expired, or foreign secret `401 client_secret_invalid`. The
     * session satisfies the policy on its own, so the next read is unlocked.
     */
    exchangeClientSecret: (id: string, clientSecret: string, options: { signal?: AbortSignal } = {}): Promise<ExchangeClientSecret> =>
      request<ExchangeClientSecret>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/session`,
        { method: "POST", body: { client_secret: clientSecret }, ...payerOptions(options) },
      ),
  };

  /**
   * The payer's wallet attestation. Once the session satisfies the request's
   * policy (at once, for a permissionless request), `challenge` returns the
   * EIP-712 document the wallet must sign, and `attest` hands the signature
   * back. The signature binds that wallet to the request and derives its
   * payment address; only transfers from that wallet count, and anything
   * Payday returns goes back to it. These writes answer cross-origin requests
   * from the hosted checkout only.
   */
  readonly wallet = {
    /**
     * Mint the challenge for `wallet`. For a permissionless request with no
     * session yet, the response carries a fresh `payer_session` to keep.
     * Answers `409 wallet_already_bound` once a wallet is bound.
     */
    challenge: (id: string, wallet: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<WalletChallenge> =>
      request<WalletChallenge>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/wallet/challenge`,
        { method: "POST", body: { wallet }, ...payerOptions(options) },
      ),
    /**
     * Submit the wallet's signature over the challenge's typed data. Answers
     * the unlocked payment, now carrying `address` and `payer_wallet`;
     * `401 wallet_signature_invalid` for a signature that does not recover to
     * `wallet`, `409 wallet_challenge_required` when no challenge is
     * outstanding, `409 wallet_already_bound` when another wallet won.
     */
    attest: (id: string, wallet: string, signature: string, payerSession: string, options: { signal?: AbortSignal } = {}): Promise<PayerPayment> =>
      request<PayerPayment>(
        this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}/wallet/attest`,
        { method: "POST", body: { wallet, signature }, ...payerOptions({ ...options, payerSession }) },
      ),
  };
}

function payerOptions(options: { signal?: AbortSignal; payerSession?: string; accept?: string }): RequestOptions {
  return {
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    ...(options.accept === undefined ? {} : { accept: options.accept }),
    ...(options.payerSession === undefined ? {} : { headers: { [PAYER_SESSION_HEADER]: options.payerSession } }),
  };
}

function query(params: object): string {
  const values = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) if (value !== undefined) values.set(key, String(value));
  const encoded = values.toString();
  return encoded ? `?${encoded}` : "";
}
