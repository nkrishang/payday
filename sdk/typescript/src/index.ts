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

export interface ExpectedIdentity {
  first_name: string;
  last_name: string;
}

export type PayerPolicyMode = "permissionless" | "verified_email" | "verified_identity" | "verified_identity_unattributed";

/**
 * Who may pay, and what they must prove first. Every verified mode names the
 * expected mailbox; `verified_identity` additionally asserts the legal name a
 * document check must match. Assertions are merchant-supplied and immutable
 * once the invoice is issued.
 */
export type PayerPolicy =
  | { mode: "permissionless" }
  | { mode: "verified_email"; expected_email: string }
  | { mode: "verified_identity"; expected_email: string; expected_identity: ExpectedIdentity }
  | { mode: "verified_identity_unattributed"; expected_email: string };

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
  address: string;
  address_explorer_url: string | null;
  payout_address: string;
  /**
   * Payday's custodial recovery wallet, committed into the payment address.
   * Overpayment remainders, expired balances, and late transfers land there
   * and are returned by the operator after manual review; merchants cannot
   * choose it.
   */
  recovery_address: string;
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
  metadata: Record<string, JsonValue>;
  /** Full policy including assertions; merchant-only, never on the payer route. */
  payer_policy: PayerPolicy;
  attachment: AttachmentDescriptor | null;
  verification_completed_at: string | null;
  /** Set when finalized funds arrived before the policy's verification completed. */
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
  self_settlement: { factory: string; salt: string };
  attention: { code: string; message: string; action: string } | null;
  transfers: Transfer[];
  indexer_freshness: { last_indexed_block: string | null; last_finalized_block: string | null; cursor_updated_at: string | null };
}

export type VerificationFactStatus = "not_required" | "pending" | "approved" | "declined";

/** Each fact is tracked separately even though only four presets are offered. */
export interface VerificationRequirements {
  email: VerificationFactStatus;
  document: VerificationFactStatus;
  liveness: VerificationFactStatus;
  identity_match: VerificationFactStatus;
  complete: boolean;
}

/** The policy as the payer may see it: the mode and a masked mailbox hint such as `a****@e***.com`. */
export interface PayerPolicySummary { mode: PayerPolicyMode; expected_email_hint: string | null }

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
  address: string | null;
  address_explorer_url: string | null;
  /** EIP-681 request for the amount still due; null while locked or once not payable. */
  payment_uri: string | null;
  invoice: PayerInvoiceDetails | null;
}

export interface PaymentSummary {
  id: string;
  heading: string | null;
  bill_to_name: string;
  reference: string | null;
  metadata: Record<string, JsonValue>;
  payer_policy_mode: PayerPolicyMode;
  customer_id: string | null;
  has_attachment: boolean;
  verification_completed_at: string | null;
  likely_unsolicited_at: string | null;
  created_at: string;
  status: PaymentStatus;
  amount: string;
  received: string;
  cancellation_requested_at: string | null;
}
export interface ListPaymentsParams { starting_after?: string; status?: PaymentStatus; reference?: string; limit?: number }
export interface PaymentPage { payments: PaymentSummary[]; next_cursor: string | null }
export interface Transfer {
  transaction_hash: string; explorer_url: string | null; sender: string; amount: string; amount_base_units: string;
  block: string; timestamp: string; disposition: "credited" | "late" | "zero"; collected: boolean;
}
export interface CancelPaymentResponse { payment: Payment; advisory: string }

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

/** A presigned slot to PUT one PDF into; `headers` must be sent verbatim with the PUT. */
export interface AttachmentUpload {
  id: string;
  upload_url: string;
  headers: Record<string, string>;
  expires_at: string;
}

export interface AttachmentCommitment { id: string; byte_length: string; sha256: string }

/** The exact document hashed at issuance; every string is canonical (decimal amounts, EIP-55 addresses). */
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
  recovery_address: string;
  factory_address: string;
}

export interface ProofTransfer {
  transaction_hash: string;
  sender: string;
  recipient: string;
  amount_base_units: string;
  block_number: string;
}

/**
 * What Payday signs about a verification outcome. The issuance commitment
 * (attribution hash, chain, CREATE3 address) is in the signed payload so the
 * attestation vouches for this invoice only, not for any proof reusing its id.
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
  payer_policy_mode: PayerPolicyMode;
  result: string;
  verified_at: string | null;
}

/** Payday-attested, not address-committed: verification happens after issuance. */
export interface SignedVerificationAttestation {
  payload: VerificationAttestationPayload;
  signer: string;
  signature: string;
}

/**
 * Offline-verifiable record tying the issued invoice to its payment address,
 * the transfers that paid it, and the transaction that settled it.
 * `payday proof verify` checks it without Payday.
 */
export interface ProofOfPayment {
  version: string;
  payment_id: string;
  canonical_issuance_snapshot: CanonicalIssuanceSnapshot;
  canonicalization: string;
  attribution_nonce: string;
  attribution_hash: string;
  salt: string;
  chain_id: string;
  factory_address: string;
  payment_address: string;
  token_address: string;
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

interface PaydayClientCommonOptions {
  baseUrl?: string;
  fetch?: typeof globalThis.fetch;
}

/**
 * Exactly one credential: a server-side API key, or the short-lived Auth0
 * access token a signed-in dashboard holds. Both travel as the same bearer
 * header; the API tells them apart.
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

  constructor(message: string, code: string, status: number, requestId?: string) {
    super(message);
    this.name = "PaydayError";
    this.code = code;
    this.status = status;
    this.requestId = requestId;
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
  let wire: { error?: { code?: string; message?: string }; request_id?: string } | undefined;
  try { wire = text ? JSON.parse(text) : undefined; } catch { wire = undefined; }
  return new PaydayError(
    wire?.error?.message ?? response.statusText ?? "Payday API request failed",
    wire?.error?.code ?? "http_error", response.status,
    wire?.request_id ?? response.headers.get("x-request-id") ?? undefined,
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
  };

  readonly customers = {
    create: (customer: CreateCustomer): Promise<Customer> =>
      this.request("/v1/customers", { method: "POST", body: customer }),
    get: (id: string): Promise<Customer> =>
      this.request(`/v1/customers/${encodeURIComponent(id)}`),
    list: (params: ListCustomersParams = {}): Promise<CustomerPage> =>
      this.request(`/v1/customers${query(params)}`),
    update: (id: string, customer: UpdateCustomer): Promise<Customer> =>
      this.request(`/v1/customers/${encodeURIComponent(id)}`, { method: "PATCH", body: customer }),
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
     * Absolute URL of the payment's QR, an SVG of the same EIP-681 request the
     * page shows. Serve it through an `<img>`: it needs no CORS, and it answers
     * `410 payment_not_payable` the moment the address must stop being shown.
     */
    qrUrl: (id: string): string =>
      `${this.baseUrl}/v1/payer/payments/${encodeURIComponent(id)}/qr`,
  };
}

function payerOptions(options: { signal?: AbortSignal; payerSession?: string }): RequestOptions {
  return {
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    ...(options.payerSession === undefined ? {} : { headers: { [PAYER_SESSION_HEADER]: options.payerSession } }),
  };
}

function query(params: object): string {
  const values = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) if (value !== undefined) values.set(key, String(value));
  const encoded = values.toString();
  return encoded ? `?${encoded}` : "";
}
