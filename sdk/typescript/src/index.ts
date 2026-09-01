export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type PaymentStatus = "awaiting_payment" | "partially_paid" | "paid" | "settled" | "expired" | "returned" | "needs_attention";

export interface Chain { id: string; name: string }
export interface Token { symbol: string; address: string; decimals: number }
export interface AsOf { block: string; at: string }
export interface CreatePayment {
  amount: string;
  payout_address: string;
  refund_address?: string;
  chain_id?: string;
  token_address?: string;
  expires_in?: number;
  expires_at?: string;
  memo?: string;
  reference?: string;
  metadata?: Record<string, JsonValue>;
}
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
  refund_address: string;
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
  reference: string | null;
  metadata: Record<string, JsonValue>;
  memo: string | null;
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

/**
 * The narrowed projection served to anyone holding a payment link.
 *
 * Deliberately carries no merchant data: no payout or refund address, no memo,
 * reference, or metadata. The payment page is world-readable, so this is the
 * only payment shape safe to render on it.
 */
export interface PayerPayment {
  id: string;
  chain: Chain;
  token: Token;
  amount: string;
  amount_base_units: string;
  received: string;
  received_base_units: string;
  remaining: string;
  remaining_base_units: string;
  address: string;
  expires_at: string;
  /** Gateway wall-clock unix seconds, so a countdown never trusts the payer's device. */
  server_timestamp: string;
  status: PaymentStatus;
  /** Whether the gateway still considers this address payable. */
  payable: boolean;
  /** EIP-681 request for the amount still due; null once the payment is not payable. */
  payment_uri: string | null;
  address_explorer_url: string | null;
  settlement_tx_hash: string | null;
  settlement_explorer_url: string | null;
  /** Safety guidance shown only when payout needs operator attention. */
  payer_message: string | null;
}

export interface PaymentSummary { id: string; memo: string | null; reference: string | null; metadata: Record<string, JsonValue>; created_at: string; status: PaymentStatus; amount: string; received: string; cancellation_requested_at: string | null }
export interface ListPaymentsParams { starting_after?: string; status?: PaymentStatus; reference?: string; limit?: number }
export interface PaymentPage { payments: PaymentSummary[]; next_cursor: string | null }
export interface Transfer {
  transaction_hash: string; explorer_url: string | null; sender: string; amount: string; amount_base_units: string;
  block: string; timestamp: string; disposition: "credited" | "late" | "zero"; collected: boolean;
}
export interface CancelPaymentResponse { payment: Payment; advisory: string }
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

export interface PaydayClientOptions {
  apiKey: string;
  baseUrl?: string;
  fetch?: typeof globalThis.fetch;
}

export interface PaydayPayerClientOptions {
  baseUrl?: string;
  fetch?: typeof globalThis.fetch;
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

interface RequestOptions {
  method?: string;
  body?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
}

/**
 * Every Payday response is `Cache-Control: no-store`, so requests opt out of
 * caching explicitly. This also stops frameworks that patch `fetch` with a
 * caching default (Next.js) from serving a stale payment.
 */
async function request<T>(
  fetcher: typeof globalThis.fetch,
  baseUrl: string,
  path: string,
  options: RequestOptions = {},
): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json", ...options.headers };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetcher(`${baseUrl}${path}`, {
    method: options.method ?? "GET",
    headers,
    cache: "no-store",
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
  });
  const text = await response.text();
  let data: unknown;
  try { data = text ? JSON.parse(text) : undefined; } catch { data = undefined; }
  if (!response.ok) {
    const wire = data as { error?: { code?: string; message?: string }; request_id?: string } | undefined;
    throw new PaydayError(
      wire?.error?.message ?? response.statusText ?? "Payday API request failed",
      wire?.error?.code ?? "http_error", response.status,
      wire?.request_id ?? response.headers.get("x-request-id") ?? undefined,
    );
  }
  return data as T;
}

function normalizeBaseUrl(baseUrl: string | undefined): string {
  return (baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
}

export class PaydayClient {
  private readonly apiKey: string;
  private readonly baseUrl: string;
  private readonly fetcher: typeof globalThis.fetch;

  constructor(options: PaydayClientOptions) {
    if (!options.apiKey) throw new TypeError("apiKey is required");
    this.apiKey = options.apiKey;
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

  private request<T>(path: string, options: { method?: string; body?: unknown; idempotencyKey?: string } = {}): Promise<T> {
    const headers: Record<string, string> = { Authorization: `Bearer ${this.apiKey}` };
    if (options.idempotencyKey !== undefined) headers["Idempotency-Key"] = options.idempotencyKey;
    return request<T>(this.fetcher, this.baseUrl, path, {
      headers,
      ...(options.method === undefined ? {} : { method: options.method }),
      ...(options.body === undefined ? {} : { body: options.body }),
    });
  }
}

/**
 * Keyless client for the public payer routes behind a `payment_url`.
 *
 * A payment link is intentionally open: anyone holding it may view the payment
 * and fulfil it. These routes accept no API key, so never pass a secret here.
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
    get: (id: string, options: { signal?: AbortSignal } = {}): Promise<PayerPayment> =>
      request<PayerPayment>(this.fetcher, this.baseUrl, `/v1/payer/payments/${encodeURIComponent(id)}`, options),

    /**
     * Absolute URL of the payment's QR, an SVG of the same EIP-681 request the
     * page shows. Serve it through an `<img>`: it needs no CORS, and it answers
     * `410 payment_not_payable` the moment the address must stop being shown.
     */
    qrUrl: (id: string): string =>
      `${this.baseUrl}/v1/payer/payments/${encodeURIComponent(id)}/qr`,
  };
}

function query(params: object): string {
  const values = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) if (value !== undefined) values.set(key, String(value));
  const encoded = values.toString();
  return encoded ? `?${encoded}` : "";
}
