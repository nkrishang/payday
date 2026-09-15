export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type DepositRequestStatus = "awaiting_deposit" | "partially_deposited" | "deposited" | "settled" | "expired" | "returned" | "needs_attention";

export interface Chain { id: string; name: string; /** The gas token's symbol on this chain. */ native_symbol: string }
/** The request's currency as one chain's contract; `symbol` is what a wallet shows there (USDT0 for USDT on Monad). */
export interface Token { symbol: string; address: string; decimals: number }
/** One network a deposit request can be paid on: the chain and its exact native USDC contract. */
export interface Network { chain: Chain; token: Token }
export interface AsOf { block: string; at: string }

/** A named party on a deposit request: the issuer or the payer. `details` is bounded free text rendered verbatim, never parsed. */
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
 * merchant-supplied and immutable once the deposit request is issued.
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
  /** 0x-prefixed lowercase hex of the stored bytes; committed into the deposit address. */
  sha256: string;
  /** Short-lived signed URL, present only on attachment reads. */
  download_url?: string;
}

export interface CreateDepositRequest {
  amount: string;
  /**
   * `USDC` (the default) or `USDT`. USDT has no 1:1 bridge for the merchant,
   * so a USDT request must pin `chain_id` to a network that serves it; the
   * payer may still pay it from another network through Relay.
   */
  currency?: string;
  /**
   * Where exactly `amount` settles. May be left out when `issuer_id` names an
   * identity with a saved payout address: its first one is used.
   */
  payout_address?: string;
  /**
   * Pin the network the payer must pay on: one of the deployment's chain ids
   * as a decimal string (`"143"`). Left out, the payer chooses among every
   * network serving the currency when they sign; required for USDT. A chain
   * Payday does not serve the currency on is refused with `422 unsupported_chain`.
   */
  chain_id?: string;
  /**
   * The issuing party as the document will carry it. May be left out when
   * `issuer_id` is given: the identity's name, contact address, and details
   * are snapshotted in its place. An inline party always wins.
   */
  issuer?: Party;
  /**
   * The paying party. May be left out when `customer_id` is given: the saved
   * customer is snapshotted in its place. An inline party always wins.
   */
  payer?: Party;
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
  /** A finalized attachment from `attachments.upload`; one PDF per deposit request. */
  attachment_id?: string;
  expires_in?: number;
  expires_at?: string;
}

/** The commitment that ties the issued deposit request to the deposit address. */
export interface Attribution { version: number; hash: string }

export interface DepositRequest {
  id: string;
  deposit_url: string;
  status: DepositRequestStatus;
  /**
   * Every network the payer may pay on; the request commits to all of them
   * and the payer picks one when they sign their wallet attestation. One
   * entry when the merchant pinned the network with `chain_id`.
   */
  networks: Network[];
  /**
   * The payment's network: the pinned one from issuance, else the one the
   * payer chose once a wallet is bound; null before either.
   */
  chain: Chain | null;
  currency: string;
  token: Token | null;
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
  payer: Party;
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
   * `merchant_session` only, and only on the `201` that issued the deposit request:
   * the first single-use client secret, valid for fifteen minutes. Absent on
   * every later read and on idempotent replays; the API stores only its hash.
   * Send the payer to `checkoutUrl(depositRequest, client_secret)`; mint another with
   * `depositRequests.createClientSecret` when they come back.
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
  deposited_at: string | null;
  deposited_at_block: string | null;
  settled_at: string | null;
  settled_block: string | null;
  expired_at: string | null;
  cancellation_requested_at: string | null;
  settlement_tx_hash: string | null;
  settlement_explorer_url: string | null;
  as_of: AsOf | null;
  /** What a third party needs to execute the contract on the chosen chain; null until the address exists. */
  self_settlement: { chain_id: string; factory: string; salt: string } | null;
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

/** A fresh single-use client secret for a `merchant_session` deposit request. Returned once; stored hashed. */
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
 * The URL to send an authenticated payer to for a `merchant_session` deposit request:
 * the deposit page with the client secret in the fragment, which never reaches
 * a server log, a Referer header, or an analytics beacon. Redirect to it or
 * open it in a frame; never write it to a log.
 */
export function checkoutUrl(depositRequest: Pick<DepositRequest, "deposit_url">, clientSecret: string): string {
  if (!clientSecret) throw new TypeError("clientSecret is required");
  return `${depositRequest.deposit_url}#${CLIENT_SECRET_FRAGMENT_KEY}=${encodeURIComponent(clientSecret)}`;
}

/** The fragment key the hosted checkout reads a merchant preview session from. */
export const PREVIEW_SESSION_FRAGMENT_KEY = "ps";

/**
 * The URL to preview a deposit request's own payer view as its issuing
 * merchant, unlocked exactly as a verified payer would see it: the preview
 * session (`depositRequests.previewSession`) in the fragment, the same way
 * `checkoutUrl` carries a client secret — never reaching a server log, a
 * Referer header, or an analytics beacon.
 */
export function previewUrl(depositRequest: Pick<DepositRequest, "deposit_url">, previewSession: string): string {
  if (!previewSession) throw new TypeError("previewSession is required");
  return `${depositRequest.deposit_url}#${PREVIEW_SESSION_FRAGMENT_KEY}=${encodeURIComponent(previewSession)}`;
}

/** The policy as the payer may see it: the mode and a masked mailbox hint such as `a****@e***.com`. */
export interface PayerPolicySummary { mode: PayerPolicyMode; expected_email_hint: string | null }

/** A payer session minted by `verification.startEmail`; the token is opaque and stored hashed by the API. */
export interface StartEmailVerification { payer_session: string; expires_at: string }

/** What a session (or, without one, the deposit request) has established. */
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

/** The merchant's verification view of one deposit request: each fact on its own and every attempt. */
export interface VerificationDetail {
  payer_policy_mode: PayerPolicyMode;
  verification_completed_at: string | null;
  likely_unsolicited_at: string | null;
  facts: VerificationRequirements;
  attempts: VerificationAttempt[];
}

/** Deposit request content that a gated request withholds until verification completes. */
export interface PayerDepositRequestDetails {
  amount: string;
  amount_base_units: string;
  payer: Party;
  notes: string | null;
  reference: string | null;
  attachment: AttachmentDescriptor | null;
}

/**
 * The narrowed projection served to anyone holding a deposit link.
 *
 * Deliberately carries no merchant data: no payout or recovery address, no
 * metadata, customer, or policy assertions. The deposit page is world-readable,
 * so this is the only deposit request shape safe to render on it.
 *
 * Gated deposit requests disclose progressively. Until `content_unlocked` is true the
 * mechanics (`chain`, `token`, amounts, `address`, `deposit_uri`) and `details`
 * are null; only the issuer name, heading, status, and requirement state show.
 */
export interface PayerDepositRequest {
  id: string;
  issuer_name: string;
  heading: string | null;
  payer_policy: PayerPolicySummary;
  requirements: VerificationRequirements;
  status: DepositRequestStatus;
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
  /** The request's currency (`USDC`, `USDT`); null while locked. */
  currency: string | null;
  /** The networks the payer may choose from; null while locked. One entry when pinned. */
  networks: Network[] | null;
  /**
   * The payment's network once known (pinned at issuance, or chosen when a
   * wallet is bound) and the content is unlocked.
   */
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
  deposit_uri: string | null;
  details: PayerDepositRequestDetails | null;
  /**
   * Whether the payer may pay from another network through Relay: the
   * deployment offers it, the address exists, and the request is payable.
   */
  relay_available: boolean;
  /** The newest cross-chain payment quoted for this request, if any; null while locked. */
  relay: PayerRelayIntent | null;
}

/** A stablecoin a payer may send from a Relay origin network. */
export interface RelayOriginToken {
  /** `USDC` or `USDT`. */
  currency: string;
  /** The contract's own symbol, as the payer's wallet shows it. */
  symbol: string;
  address: string;
  decimals: number;
}

/** A network a payer may pay from through Relay, with the stablecoins they may send there. */
export interface RelayOriginChain {
  /** Decimal chain id. */
  chain_id: string;
  name: string;
  native_symbol: string | null;
  /** What the payer may send from this network; Relay swaps it into the request's currency. At least one entry. */
  tokens: RelayOriginToken[];
  explorer_url: string | null;
  icon_url: string | null;
  /** A public RPC, so a wallet that lacks the network can be asked to add it. */
  rpc_url: string | null;
}

export interface RelayOriginChains {
  chains: RelayOriginChain[];
}

/** One transaction the attested wallet sends on the origin network. */
export interface RelayTransaction {
  /** Decimal chain id: the origin network. */
  chain_id: string;
  to: string;
  /** `0x` hex calldata. */
  data: string;
  /** Decimal wei. */
  value: string;
  /** Relay's gas estimate, when it gives one. */
  gas: string | null;
}

export interface RelayQuoteStep {
  /** `approve` or `deposit`. */
  id: string;
  transaction: RelayTransaction;
}

/**
 * A quote for paying the amount still due from another network. The steps
 * are transactions for the attested wallet to send on the origin network, in
 * order; the last one is the deposit Relay fills against. Exactly
 * `amount_out` lands on the payment address.
 */
export interface RelayQuote {
  /** The `rli_` id to report the origin transaction against. */
  id: string;
  request_id: string;
  origin: RelayOriginChain;
  /** The stablecoin the payer sends on the origin network. */
  origin_token: RelayOriginToken;
  /** What the payer sends, in `origin_token`. */
  amount_in: string;
  amount_in_base_units: string;
  amount_out: string;
  amount_out_base_units: string;
  relayer_fee_usd: string | null;
  time_estimate_seconds: number;
  /** Ask for another quote after this. */
  expires_at: string;
  steps: RelayQuoteStep[];
}

export type RelayIntentStatus = "quoted" | "sent" | "filled" | "failed" | "refunded" | "expired";

/** The cross-chain payment the page is following. */
export interface PayerRelayIntent {
  id: string;
  status: RelayIntentStatus;
  origin_chain_id: string;
  origin_transaction_hash: string | null;
  /** The destination transaction that delivered the funds, once filled. */
  fill_transaction_hash: string | null;
  created_at: string;
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

/** A wallet challenge: the session it belongs to, the network it is for, and the document to sign. */
export interface WalletChallenge {
  payer_session: string;
  expires_at: string;
  /** The network the payer chose; `typed_data.domain.chainId` names it too. */
  chain: Chain;
  typed_data: PayerAttestationTypedData;
}

export interface DepositRequestSummary {
  id: string;
  deposit_url: string;
  heading: string | null;
  payer_name: string;
  reference: string | null;
  metadata: Record<string, JsonValue>;
  payer_policy_mode: PayerPolicyMode;
  customer_id: string | null;
  issuer_id: string | null;
  has_attachment: boolean;
  verification_completed_at: string | null;
  likely_unsolicited_at: string | null;
  created_at: string;
  updated_at: string;
  expires_at: string;
  status: DepositRequestStatus;
  amount: string;
  received: string;
  /** `USDC` or `USDT`. */
  currency: string;
  cancellation_requested_at: string | null;
}
/** Verification is a separate fact from the deposit request's status, so it filters separately. */
export type VerificationFilter = "not_required" | "pending" | "verified" | "likely_unsolicited";
export interface ListDepositRequestsParams {
  starting_after?: string;
  status?: DepositRequestStatus;
  reference?: string;
  /** Only requests billed to this customer. */
  customer_id?: string;
  /** Only requests issued under this identity. */
  issuer_id?: string;
  verification?: VerificationFilter;
  limit?: number;
}
export interface DepositRequestPage { deposit_requests: DepositRequestSummary[]; next_cursor: string | null }
export interface Transfer {
  transaction_hash: string; explorer_url: string | null; sender: string; amount: string; amount_base_units: string;
  block: string; timestamp: string; disposition: "credited" | "late" | "zero"; collected: boolean;
  /** Present when Relay's solver sent it for a cross-chain payment the attested wallet made. */
  relay?: TransferRelay;
}
/** The origin of a transfer Relay delivered: what the attested wallet sent. */
export interface TransferRelay {
  request_id: string;
  /** Decimal chain id the wallet paid on. */
  origin_chain_id: string;
  origin_transaction_hash: string | null;
}
export interface TransferList { transfers: Transfer[] }
/** Internal: the dashboard onboarding walkthrough's one real demo transfer. */
export interface OnboardingDepositResponse { payer_session: string; tx_hash: string }

/** A saved payout wallet, EIP-55 checksummed and unique per account. */
export interface PayoutAddress {
  id: string;
  address: string;
  label: string | null;
  created_at: string;
}

/**
 * A saved issuer identity: the party a deposit request is issued under, its contact
 * mailbox, and the wallets it may settle to. Issuance is unchanged — a deposit
 * request still carries its own `issuer` and `payout_address` snapshot — so
 * editing an identity never touches a request already issued.
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
/**
 * A partial update: a field left out keeps its value; `details: null` clears
 * it. A changed `contact_email` clears the verification, so a rename alone
 * never touches a proven mailbox.
 */
export interface UpdateIssuer { name?: string; contact_email?: string; details?: string | null }
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
/** A partial update: a field left out keeps its value; `email: null` or `details: null` clears it. */
export interface UpdateCustomer { name?: string; email?: string | null; details?: string | null }
export interface ListCustomersParams { starting_after?: string; limit?: number }
export interface CustomerPage { customers: Customer[]; next_cursor: string | null }
/** One currency's totals: base units, like a deposit request's own `amount_base_units`, scaled for display by that currency's decimals. */
export interface CustomerCurrencyTotal {
  currency: string;
  request_count: number;
  collected_base_units: string;
  pending_base_units: string;
}
/** Totals never add across currencies, so there is one entry per currency the customer has been asked for. */
export interface CustomerStats {
  request_count: number;
  totals: CustomerCurrencyTotal[];
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
  schema: "payday.invoice.v4";
  canonicalization: "RFC8785";
  issuer: Party;
  bill_to: Party;
  /** The currency's wire code (`USDC`, `USDT`); every network below is that currency's contract on its chain. */
  currency: string;
  /** Decimal: base units per whole unit, so `amount_base_units` reads without a registry. */
  decimals: string;
  amount_base_units: string;
  notes: string | null;
  heading: string | null;
  reference: string | null;
  expiration_timestamp: string;
  payer_policy: PayerPolicy;
  attachment: AttachmentCommitment | null;
  /** Every network the request may be paid on, ordered by chain id. */
  networks: Array<{ chain_id: string; token_address: string; factory_address: string }>;
  receiver_address: string;
}

/**
 * The payer's wallet attestation as the proof carries it: the exact typed
 * data the wallet signed (under the chosen chain's domain), its EIP-712
 * digest, and the signature. The proof's salt is
 * `keccak256("PAYDAY_SALT_V3" || attribution_hash || digest)` and the wallet
 * is the address's recovery term.
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
  /** The attested wallet, or Relay's solver for a relayed transfer. */
  sender: string;
  recipient: string;
  amount_base_units: string;
  block_number: string;
  /**
   * Present when Relay's solver made the transfer for a cross-chain payment
   * the attested wallet sent. A verifier accepts it only when the same block
   * appears in the attestation's `relay_fills` and `origin_sender` is the
   * attested wallet.
   */
  relay?: RelayAttribution;
}

/** Where a relayed transfer's funds came from; vouched for by the attestation. */
export interface RelayAttribution {
  /** Relay's request id, `0x` hex, 32 bytes. */
  request_id: string;
  /** Decimal chain id the wallet paid on. */
  origin_chain_id: string;
  /** The transaction the wallet sent there. */
  origin_transaction_hash: string;
  /** Always the attested wallet. */
  origin_sender: string;
  /** Attribution verified from an origin-chain transaction receipt. */
  attribution_source: "receipt";
}

/** A relayed transfer as the attestation vouches for it. */
export interface AttestedRelayFill extends RelayAttribution {
  transaction_hash: string;
  log_index: string;
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
  /** `0x` hex, 32 bytes: the attribution hash of the canonical deposit request. */
  attribution_hash: string;
  /** Decimal: the chain the payer chose, one of the snapshot's networks. */
  chain_id: string;
  /** EIP-55 checksummed CREATE3 payment address. */
  payment_address: string;
  payer_wallet: string;
  wallet_nonce: string;
  /**
   * The transfers Relay's solver made for cross-chain payments the attested
   * wallet sent, each with the origin Payday verified; absent when every
   * transfer came from the wallet itself.
   */
  relay_fills?: AttestedRelayFill[];
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
 * Offline-verifiable record tying the issued deposit request to the wallet its payer
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
  /** The network the payer chose among the snapshot's `networks`; the factory and token are that network's. */
  chain_id: string;
  factory_address: string;
  payment_address: string;
  token_address: string;
  /** The payer's attested wallet. */
  recovery_address: string;
  /**
   * The fulfilment transaction (the deposit request's `settlement_tx_hash`) that
   * forwarded the funds to the receiver; not one of `transfers`, which are the
   * payer's USDC transfers into the deposit address.
   */
  settlement_transaction_hash: string;
  transfers: ProofTransfer[];
  verification: SignedVerificationAttestation;
}

/** One entry per supported network. */
export interface ServiceStatus {
  chains: Array<{
    id: string;
    name: string;
    finalized_block: string | null;
    finalized_at: string | null;
    indexer: { cursor_block: string | null; cursor_at: string | null; lag_blocks: number | null };
    sweeper: { state: string; queued: number };
  }>;
}
export interface Webhook {
  id: string;
  url: string;
  created_at: string;
  /** Set once disabled: no longer listed, receives nothing, but still readable with its deliveries. */
  disabled_at: string | null;
  /** The signing secret, on the response to `webhooks.add` only. */
  secret?: string;
}
export interface WebhookList { webhooks: Webhook[] }
export interface TestDelivery { delivery_id: string }
export interface DeliveryAttempt {
  number: number; attempted_at: string; duration_ms: number; status: number | null; error: string | null;
}
export interface WebhookDelivery {
  id: string; event_id: string; endpoint_id: string; state: "pending" | "delivered" | "failed"; attempt_count: number;
  next_attempt_at: string; delivered_at: string | null; created_at: string; attempts: DeliveryAttempt[];
}
export interface ListWebhookDeliveriesParams {
  /** Only this endpoint's deliveries, disabled or not. */
  endpoint_id?: string;
  starting_after?: string;
  limit?: number;
}
export interface WebhookDeliveryPage { deliveries: WebhookDelivery[]; next_cursor: string | null }

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
 * caching default (Next.js) from serving a stale deposit request.
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

// --- Withdrawals ---

/** EIP-712 typed data for one withdrawal leg: an EIP-3009 authorization under the source chain's token contract. Every `uint256` is a decimal string. */
export interface WithdrawalTypedData {
  domain: { name: string; version: string; chainId: number; verifyingContract: string };
  primaryType: "TransferWithAuthorization" | "ReceiveWithAuthorization";
  types: Record<string, Array<{ name: string; type: string }>>;
  message: {
    from: string;
    to: string;
    value: string;
    validAfter: string;
    validBefore: string;
    nonce: string;
  };
}

export interface WithdrawalNoncePreimage {
  /** Circle's CCTP domain of the destination chain. */
  destination_domain: number;
  /** The destination address, as the mint recipient. */
  mint_recipient: string;
  /** `0x` hex, 32 bytes. */
  salt: string;
}

/** What the merchant signs for one leg, present while the leg awaits its signature. */
export interface WithdrawalAuthorization {
  primary_type: "TransferWithAuthorization" | "ReceiveWithAuthorization";
  typed_data: WithdrawalTypedData;
  /** When the token stops accepting the signature. */
  expires_at: string;
  /** Bridge legs: the WithdrawalForwarder the authorization pays. */
  forwarder: string | null;
  /** Bridge legs: `nonce = keccak256(abi.encode(destination_domain, bytes32(mint_recipient), salt))`. */
  nonce_preimage: WithdrawalNoncePreimage | null;
}

export type WithdrawalLegState =
  | "awaiting_signature"
  | "authorized"
  | "relaying"
  | "burned"
  | "attested"
  | "minting"
  | "completed"
  | "failed"
  | "expired"
  | "cancelled";

export interface WithdrawalLeg {
  id: string;
  /** `transfer` when the funds already sit on the destination chain, `bridge` when they cross through CCTP. */
  kind: "transfer" | "bridge";
  source_chain: Chain;
  /** The contract the leg is signed under: the withdrawal's currency on the source network. */
  token: Token;
  amount: string;
  amount_base_units: string;
  state: WithdrawalLegState;
  authorization: WithdrawalAuthorization | null;
  transfer_tx_hash: string | null;
  burn_tx_hash: string | null;
  mint_tx_hash: string | null;
  failure_reason: string | null;
}

export type WithdrawalStatus = "awaiting_signature" | "in_progress" | "completed" | "failed" | "cancelled";

export interface Withdrawal {
  id: string;
  status: WithdrawalStatus;
  /** The Payday wallet every leg is signed from. */
  wallet_address: string;
  /** `USDC` or `USDT`: what every leg moves. */
  currency: string;
  destination: { chain: Chain; address: string };
  /** One per network the withdrawal moves the currency from: every network holding USDC, the destination alone for USDT. */
  legs: WithdrawalLeg[];
  created_at: string;
  completed_at: string | null;
  cancelled_at: string | null;
  failed_at: string | null;
}

export interface WithdrawalPage {
  withdrawals: Withdrawal[];
  next_cursor: string | null;
}

export interface CreateWithdrawal {
  /** `USDC` (the default) or `USDT`. */
  currency?: string;
  destination: {
    /** Decimal chain id of one of the deployment's networks serving the currency. */
    chain_id: string;
    address: string;
  };
}

export interface LegAuthorizationInput {
  leg_id: string;
  /** `0x` hex, 65 bytes `r || s || v`, over the leg's `typed_data`. */
  signature: string;
}

export interface ListWithdrawalsParams {
  limit?: number;
  starting_after?: string;
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

  readonly depositRequests = {
    create: (depositRequest: CreateDepositRequest, idempotencyKey: string): Promise<DepositRequest> => {
      if (!idempotencyKey) throw new TypeError("idempotencyKey is required");
      return this.request("/v1/deposit-requests", { method: "POST", body: depositRequest, idempotencyKey });
    },
    get: (id: string, options: { waitForChange?: boolean; timeout?: number } = {}): Promise<DepositRequest> => {
      const params = options.waitForChange ? { wait_for: "change", timeout: options.timeout } : {};
      return this.request(`/v1/deposit-requests/${encodeURIComponent(id)}${query(params)}`);
    },
    list: (params: ListDepositRequestsParams = {}): Promise<DepositRequestPage> =>
      this.request(`/v1/deposit-requests${query(params)}`),
    /**
     * Records the cancellation and returns the deposit request with
     * `cancellation_requested_at` set. Presentation only: it cannot disable
     * the address or change the settlement terms the address commits to.
     */
    cancel: (id: string): Promise<DepositRequest> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/cancel`, { method: "POST" }),
    /**
     * Internal: the dashboard onboarding walkthrough's one real demo
     * transfer and verification. Refuses `409 onboarding_deposit_not_eligible`
     * for anything not addressed to Payday's own onboarding mailbox — not a
     * general merchant feature.
     */
    onboardingDeposit: (id: string): Promise<OnboardingDepositResponse> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/onboarding-deposit`, { method: "POST" }),
    /** Finalized transfer provenance for the deposit request. */
    transfers: (id: string): Promise<TransferList> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/transfers`),
    /** The deposit request's PDF attachment with a short-lived `download_url`. */
    attachment: (id: string): Promise<AttachmentDescriptor> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/attachment`),
    /** Payday's deterministic summary of the deposit request as a PDF; the same request always renders byte-identical bytes. */
    requestPdf: async (id: string): Promise<Blob> => {
      const response = await this.send(`/v1/deposit-requests/${encodeURIComponent(id)}/request.pdf`, { accept: "application/pdf" });
      return response.blob();
    },
    /** Available once the deposit is settled; `409 deposit_request_not_settled` before. */
    proof: (id: string): Promise<ProofOfPayment> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/proof`),
    /** Every verification attempt on the deposit request, each fact reported separately. */
    verification: (id: string): Promise<VerificationDetail> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/verification`),
    /**
     * A fresh single-use client secret for a `merchant_session` deposit request, for a
     * payer your application signs in again after the first secret was spent
     * or expired. Earlier unspent secrets stay valid until they expire.
     * `409 verification_not_required` for a permissionless deposit request,
     * `409 verification_method_not_applicable` for a `verified_email` one, and
     * `410 deposit_request_not_payable` once the request is closed without having verified.
     */
    createClientSecret: (id: string): Promise<ClientSecret> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/client-secret`, { method: "POST" }),
    /**
     * A session that opens the deposit request's own payer view exactly as a
     * verified payer would see it, for the issuing merchant to preview their
     * own request — works for every payer policy, not only
     * `merchant_session`. This is not verification: it records no attempt
     * and never marks the deposit request's own verification complete.
     * `410 deposit_request_not_payable` once the request is closed without
     * having verified.
     */
    previewSession: (id: string): Promise<StartEmailVerification> =>
      this.request(`/v1/deposit-requests/${encodeURIComponent(id)}/preview-session`, { method: "POST" }),
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
   * `depositRequests.create` still takes the party and the payout address inline and
   * snapshots them, so nothing here can change a deposit request already issued. The
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
    /** Partial: only the fields given change; a different `contact_email` clears the verification. */
    update: (id: string, issuer: UpdateIssuer): Promise<Issuer> =>
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

  /**
   * Webhook endpoints and their delivery history. A missing, malformed, or
   * foreign id is `404 webhook_not_found`; `add` throws `webhooks_unavailable`
   * (503) on a deployment with no webhook encryption key.
   */
  readonly webhooks = {
    /** Registers a credential-free public HTTPS URL; the secret is returned once. */
    add: (url: string): Promise<Webhook> => this.request("/v1/webhooks", { method: "POST", body: { url } }),
    /** Active endpoints, without secrets. */
    list: (): Promise<WebhookList> => this.request("/v1/webhooks"),
    /** One endpoint, disabled or not, without its secret. */
    get: (id: string): Promise<Webhook> => this.request(`/v1/webhooks/${encodeURIComponent(id)}`),
    /** Disables the endpoint. Idempotent; its delivery history stays readable. */
    remove: (id: string): Promise<void> =>
      this.request(`/v1/webhooks/${encodeURIComponent(id)}`, { method: "DELETE" }),
    /** Queues a `webhook.test` event for this endpoint only. */
    test: (id: string): Promise<TestDelivery> =>
      this.request(`/v1/webhooks/${encodeURIComponent(id)}/test`, { method: "POST" }),
    /** Deliveries newest first, with each one's attempt history; page with `starting_after`. */
    deliveries: (params: ListWebhookDeliveriesParams = {}): Promise<WebhookDeliveryPage> =>
      this.request(`/v1/webhook-deliveries${query(params)}`),
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

  /**
   * Withdrawals: the Payday wallet's whole balance in one currency to one
   * address. Prepare, sign, submit, poll. `create` snapshots the balances
   * into legs, each carrying the EIP-712 document to sign under that
   * chain's token contract (an EIP-3009 authorization); nothing moves until it is
   * signed. Sign with `@payday/sdk/signing` or any EIP-712 signer holding
   * the wallet's key, then `authorize`. Payday relays and pays gas; the
   * signature itself fixes where the funds may land. One withdrawal may be
   * open per account (`withdrawal_in_progress`, 409).
   */
  readonly withdrawals = {
    create: (input: CreateWithdrawal, idempotencyKey: string): Promise<Withdrawal> => {
      if (!idempotencyKey) throw new TypeError("idempotencyKey is required");
      return this.request("/v1/withdrawals", { method: "POST", body: input, idempotencyKey });
    },
    get: (id: string): Promise<Withdrawal> => this.request(`/v1/withdrawals/${encodeURIComponent(id)}`),
    /** Newest first; page with `starting_after`. */
    list: (params: ListWithdrawalsParams = {}): Promise<WithdrawalPage> =>
      this.request(`/v1/withdrawals${query(params)}`),
    /** Records signatures for any subset of the legs; every one is verified before any is stored. */
    authorize: (id: string, authorizations: LegAuthorizationInput[]): Promise<Withdrawal> =>
      this.request(`/v1/withdrawals/${encodeURIComponent(id)}/authorizations`, {
        method: "POST",
        body: { authorizations },
      }),
    /** Cancels while nothing has been relayed; signatures already given are never used. */
    cancel: (id: string): Promise<Withdrawal> =>
      this.request(`/v1/withdrawals/${encodeURIComponent(id)}/cancel`, { method: "POST" }),
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
 * Keyless client for the public payer routes behind a `deposit_url`.
 *
 * A deposit link is intentionally open: anyone holding it may view the deposit
 * request and fulfil it. These routes accept no API key, so never pass a secret here.
 * A payer session token, obtained by completing verification on a gated
 * deposit request, travels in `Payday-Payer-Session` and unlocks the withheld content.
 */
export class PaydayPayerClient {
  private readonly baseUrl: string;
  private readonly fetcher: typeof globalThis.fetch;

  constructor(options: PaydayPayerClientOptions = {}) {
    this.baseUrl = normalizeBaseUrl(options.baseUrl);
    this.fetcher = options.fetch ?? globalThis.fetch;
    if (!this.fetcher) throw new TypeError("fetch is required");
  }

  readonly depositRequests = {
    get: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<PayerDepositRequest> =>
      request<PayerDepositRequest>(this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}`, payerOptions(options)),

    /**
     * The deposit request's PDF with a short-lived `download_url`. Answers
     * `401 verification_required` while the content is still gated.
     */
    attachment: (id: string, payerSession?: string, options: { signal?: AbortSignal } = {}): Promise<AttachmentDescriptor> =>
      request<AttachmentDescriptor>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/attachment`,
        payerOptions({ ...options, ...(payerSession === undefined ? {} : { payerSession }) }),
      ),

    /**
     * The deposit request's QR: an SVG of the same EIP-681 request the page shows, for
     * the amount still due. Render it from an object URL. It answers
     * `401 verification_required` while the content is gated and
     * `410 deposit_request_not_payable` the moment the address must stop being shown.
     * The session travels in a header, never in the URL.
     */
    qr: async (id: string, payerSession?: string, options: { signal?: AbortSignal } = {}): Promise<Blob> => {
      const response = await send(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/qr`,
        payerOptions({ ...options, ...(payerSession === undefined ? {} : { payerSession }), accept: "image/svg+xml" }),
      );
      return response.blob();
    },
  };

  /**
   * Email verification for a gated deposit request. The code goes to the mailbox the
   * merchant asserted at issuance; the payer only ever types the code. These
   * writes answer cross-origin requests from the hosted checkout only.
   */
  readonly verification = {
    /** Send (or, with a session, resend) the code and get the session it belongs to. */
    startEmail: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<StartEmailVerification> =>
      request<StartEmailVerification>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/verify/email/start`,
        { method: "POST", ...payerOptions(options) },
      ),
    /** Exchange the code; answers `401 otp_invalid` for a wrong one. */
    confirmEmail: (id: string, otp: string, payerSession: string, options: { signal?: AbortSignal } = {}): Promise<VerificationStatus> =>
      request<VerificationStatus>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/verify/email/confirm`,
        { method: "POST", body: { otp }, ...payerOptions({ ...options, payerSession }) },
      ),
    /** Resume persistence after `verification_persistence_unavailable`, without reusing the spent OTP. */
    continueEmail: (id: string, continuation: string, payerSession: string, options: { signal?: AbortSignal } = {}): Promise<VerificationStatus> =>
      request<VerificationStatus>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/verify/email/confirm`,
        { method: "POST", body: { continuation }, ...payerOptions({ ...options, payerSession }) },
      ),
    /** The session's facts, or the deposit request's without a session. */
    status: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<VerificationStatus> =>
      request<VerificationStatus>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/verify`,
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
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/session`,
        { method: "POST", body: { client_secret: clientSecret }, ...payerOptions(options) },
      ),
  };

  /**
   * The payer's wallet attestation. Once the session satisfies the request's
   * policy (at once, for a permissionless request), `challenge` returns the
   * EIP-712 document the wallet must sign under the chosen network's domain,
   * and `attest` hands the signature back. The signature binds that wallet
   * and that network to the request and derives its payment address; only
   * transfers from that wallet on that chain count, and anything Payday
   * returns goes back to it. These writes answer cross-origin requests from
   * the hosted checkout only.
   */
  readonly wallet = {
    /**
     * Mint the challenge for `wallet` on `chainId`, one of the request's
     * `networks`. For a permissionless request with no session yet, the
     * response carries a fresh `payer_session` to keep. Answers
     * `422 unsupported_chain` for a chain the request does not offer and
     * `409 wallet_already_bound` once a wallet is bound.
     */
    challenge: (id: string, wallet: string, chainId: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<WalletChallenge> =>
      request<WalletChallenge>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/wallet/challenge`,
        { method: "POST", body: { wallet, chain_id: chainId }, ...payerOptions(options) },
      ),
    /**
     * Submit the wallet's signature over the challenge's typed data. Answers
     * the unlocked payment, now carrying `address` and `payer_wallet`;
     * `401 wallet_signature_invalid` for a signature that does not recover to
     * `wallet`, `409 wallet_challenge_required` when no challenge is
     * outstanding, `409 wallet_already_bound` when another wallet won.
     */
    attest: (id: string, wallet: string, signature: string, payerSession: string, options: { signal?: AbortSignal } = {}): Promise<PayerDepositRequest> =>
      request<PayerDepositRequest>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/wallet/attest`,
        { method: "POST", body: { wallet, signature }, ...payerOptions({ ...options, payerSession }) },
      ),
  };

  /**
   * Paying from another network through Relay, once the address exists and
   * while the request is payable (`relay_available`). Payday makes the quote:
   * it pins the attested wallet as the sender, the payment address as the
   * recipient, and exactly the amount still due as what lands. The page
   * sends the quote's transactions from that wallet on the origin network,
   * reports the deposit's hash, and follows `relay` on the payer view. All
   * three answer `404 relay_unavailable` on a deployment without Relay,
   * `409 wallet_required` before the address exists, and
   * `410 deposit_request_not_payable` afterwards.
   */
  readonly relay = {
    /** The networks a payer may pay from, and the stablecoins they may send on each: every one Relay takes deposits on that Payday serves, except the request's own. */
    chains: (id: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<RelayOriginChains> =>
      request<RelayOriginChains>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/relay/chains`,
        payerOptions(options),
      ),
    /**
     * A quote from `originToken` on `originChainId` (the network's USDC when no
     * token is given): one of the addresses `chains` offers there, swapped by
     * Relay into the request's currency. Answers `422 relay_unsupported_origin`
     * for a network or token not offered and `502 relay_quote_failed` when
     * Relay has no route; ask again after `expires_at`.
     */
    quote: (id: string, originChainId: string, options: { originToken?: string; signal?: AbortSignal; payerSession?: string } = {}): Promise<RelayQuote> =>
      request<RelayQuote>(
        this.fetcher, this.baseUrl, `/v1/payer/deposit-requests/${encodeURIComponent(id)}/relay/quotes`,
        {
          method: "POST",
          body: { origin_chain_id: originChainId, ...(options.originToken ? { origin_token: options.originToken } : {}) },
          ...payerOptions(options),
        },
      ),
    /**
     * The wallet sent the quote's deposit: report its hash. Late and repeated
     * reports are safe. Answers the payer view with `relay.status` `sent`;
     * `409 relay_report_conflict` when the report conflicts with an existing one.
     */
    sent: (id: string, quoteId: string, transactionHash: string, options: { signal?: AbortSignal; payerSession?: string } = {}): Promise<PayerDepositRequest> =>
      request<PayerDepositRequest>(
        this.fetcher, this.baseUrl,
        `/v1/payer/deposit-requests/${encodeURIComponent(id)}/relay/quotes/${encodeURIComponent(quoteId)}/sent`,
        { method: "POST", body: { transaction_hash: transactionHash }, ...payerOptions(options) },
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
