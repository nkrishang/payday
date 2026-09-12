/**
 * The withdrawal integration, once, for both the docs page and the prompt a
 * reader hands their agent. The snippets below are the page's code tabs; the
 * prompt embeds the same strings, and a test holds the two together, so the
 * agent's brief can never lag the page.
 */

export const WITHDRAW_TS = `import { PaydayClient } from "@payday/sdk";
import { privateKeySigner, signWithdrawal } from "@payday/sdk/signing"; // needs viem installed

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });
// The Payday wallet key, exported once from the dashboard's Account section.
const signer = await privateKeySigner(process.env.PAYDAY_WALLET_KEY!);

// 1. Prepare: one leg per network the wallet holds USDC on.
let withdrawal = await payday.withdrawals.create(
  { destination: { chain_id: "8453", address: "0x1111111111111111111111111111111111111111" } },
  crypto.randomUUID(),
);

// 2. Sign: checks every document against its leg, then signs it.
const { authorizations } = await signWithdrawal(withdrawal, signer, {
  verifyNonce: signer.verifyNonce,
});

// 3. Submit, then 4. poll until every leg has landed.
withdrawal = await payday.withdrawals.authorize(withdrawal.id, authorizations);
while (withdrawal.status === "in_progress") {
  await new Promise((resolve) => setTimeout(resolve, 10_000));
  withdrawal = await payday.withdrawals.get(withdrawal.id);
}
console.log(withdrawal.status, withdrawal.legs.map((leg) => [leg.state, leg.mint_tx_hash ?? leg.transfer_tx_hash]));`;

export const WITHDRAW_RUST = `// Cargo.toml: alloy = { version = "1", features = ["signer-local", "dyn-abi"] },
// reqwest = { version = "0.12", features = ["json"] }, serde_json, uuid = { features = ["v4"] }, hex
use alloy::dyn_abi::TypedData;
use alloy::signers::{Signer, local::PrivateKeySigner};
use serde_json::{Value, json};

let api = "https://api.payday.sh";
let auth = format!("Bearer {}", std::env::var("PAYDAY_API_KEY")?);
// The Payday wallet key, exported once from the dashboard's Account section.
let signer: PrivateKeySigner = std::env::var("PAYDAY_WALLET_KEY")?.parse()?;
let http = reqwest::Client::new();

// 1. Prepare.
let mut withdrawal: Value = http
    .post(format!("{api}/v1/withdrawals"))
    .header("Authorization", &auth)
    .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
    .json(&json!({ "destination": { "chain_id": "8453", "address": "0x1111111111111111111111111111111111111111" } }))
    .send().await?.error_for_status()?.json().await?;

// 2. Sign each leg awaiting a signature. Check the document against the leg
//    first (amount, payee, chain, nonce commitment): see the checklist.
let mut authorizations = Vec::new();
for leg in withdrawal["legs"].as_array().unwrap_or(&Vec::new()) {
    if leg["state"] != "awaiting_signature" { continue; }
    let typed: TypedData = serde_json::from_value(leg["authorization"]["typed_data"].clone())?;
    let signature = signer.sign_hash(&typed.eip712_signing_hash()?).await?;
    authorizations.push(json!({
        "leg_id": leg["id"],
        "signature": format!("0x{}", hex::encode(signature.as_bytes())),
    }));
}

// 3. Submit, then 4. poll.
let id = withdrawal["id"].as_str().unwrap_or_default().to_owned();
withdrawal = http
    .post(format!("{api}/v1/withdrawals/{id}/authorizations"))
    .header("Authorization", &auth)
    .json(&json!({ "authorizations": authorizations }))
    .send().await?.error_for_status()?.json().await?;
while withdrawal["status"] == "in_progress" {
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    withdrawal = http.get(format!("{api}/v1/withdrawals/{id}"))
        .header("Authorization", &auth)
        .send().await?.error_for_status()?.json().await?;
}
println!("{}", withdrawal["status"]);`;

export const WITHDRAW_GO = `// go get github.com/ethereum/go-ethereum
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"os"
	"time"

	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/signer/core/apitypes"
	"github.com/google/uuid"
)

type leg struct {
	ID            string \`json:"id"\`
	State         string \`json:"state"\`
	Authorization *struct {
		TypedData json.RawMessage \`json:"typed_data"\`
	} \`json:"authorization"\`
}
type withdrawal struct {
	ID     string \`json:"id"\`
	Status string \`json:"status"\`
	Legs   []leg  \`json:"legs"\`
}

func call(method, path string, body any, out any) error {
	var payload []byte
	if body != nil {
		payload, _ = json.Marshal(body)
	}
	req, _ := http.NewRequest(method, "https://api.payday.sh"+path, bytes.NewReader(payload))
	req.Header.Set("Authorization", "Bearer "+os.Getenv("PAYDAY_API_KEY"))
	req.Header.Set("Content-Type", "application/json")
	if method == http.MethodPost && path == "/v1/withdrawals" {
		req.Header.Set("Idempotency-Key", uuid.NewString())
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	return json.NewDecoder(res.Body).Decode(out)
}

func main() {
	// The Payday wallet key, exported once from the dashboard's Account section.
	key, _ := crypto.HexToECDSA(os.Getenv("PAYDAY_WALLET_KEY")[2:])

	// 1. Prepare.
	var w withdrawal
	_ = call(http.MethodPost, "/v1/withdrawals", map[string]any{
		"destination": map[string]string{"chain_id": "8453", "address": "0x1111111111111111111111111111111111111111"},
	}, &w)

	// 2. Sign each leg awaiting a signature. Check the document against the
	//    leg first (amount, payee, chain, nonce commitment): see the checklist.
	var authorizations []map[string]string
	for _, l := range w.Legs {
		if l.State != "awaiting_signature" || l.Authorization == nil {
			continue
		}
		var typed apitypes.TypedData
		_ = json.Unmarshal(l.Authorization.TypedData, &typed)
		hash, _, _ := apitypes.TypedDataAndHash(typed)
		sig, _ := crypto.Sign(hash, key)
		sig[64] += 27 // recovery id to v
		authorizations = append(authorizations, map[string]string{"leg_id": l.ID, "signature": "0x" + hex.EncodeToString(sig)})
	}

	// 3. Submit, then 4. poll.
	_ = call(http.MethodPost, "/v1/withdrawals/"+w.ID+"/authorizations", map[string]any{"authorizations": authorizations}, &w)
	for w.Status == "in_progress" {
		time.Sleep(10 * time.Second)
		_ = call(http.MethodGet, "/v1/withdrawals/"+w.ID, nil, &w)
	}
}`;

export const WITHDRAW_CURL = `curl -fsS "$API/v1/withdrawals" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Idempotency-Key: $(uuidgen)" \\
  -H "Content-Type: application/json" \\
  -d '{ "destination": { "chain_id": "8453", "address": "0x1111111111111111111111111111111111111111" } }'`;

/** A bridge leg as the API returns it; the vector every implementation pins. */
export const LEG_EXAMPLE = `{
  "id": "wdl_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "kind": "bridge",
  "source_chain": { "id": "143", "name": "Monad", "native_symbol": "MON" },
  "amount": "1.234567",
  "amount_base_units": "1234567",
  "state": "awaiting_signature",
  "authorization": {
    "primary_type": "ReceiveWithAuthorization",
    "typed_data": {
      "domain": {
        "name": "USDC",
        "version": "2",
        "chainId": 143,
        "verifyingContract": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603"
      },
      "primaryType": "ReceiveWithAuthorization",
      "types": {
        "EIP712Domain": [
          { "name": "name", "type": "string" },
          { "name": "version", "type": "string" },
          { "name": "chainId", "type": "uint256" },
          { "name": "verifyingContract", "type": "address" }
        ],
        "ReceiveWithAuthorization": [
          { "name": "from", "type": "address" },
          { "name": "to", "type": "address" },
          { "name": "value", "type": "uint256" },
          { "name": "validAfter", "type": "uint256" },
          { "name": "validBefore", "type": "uint256" },
          { "name": "nonce", "type": "bytes32" }
        ]
      },
      "message": {
        "from": "0x1111111111111111111111111111111111111111",
        "to": "0x2222222222222222222222222222222222222222",
        "value": "1234567",
        "validAfter": "0",
        "validBefore": "1800000000",
        "nonce": "0x18b79105e486e10f626b71939a0226c47316949b7c24ff1c397661ea861fafaa"
      }
    },
    "expires_at": "2027-01-15T08:00:00Z",
    "forwarder": "0x2222222222222222222222222222222222222222",
    "nonce_preimage": {
      "destination_domain": 6,
      "mint_recipient": "0x000000000000000000000000000000000000d00d",
      "salt": "0x0000000000000000000000000000000000000000000000000000000000000007"
    }
  },
  "transfer_tx_hash": null,
  "burn_tx_hash": null,
  "mint_tx_hash": null,
  "failure_reason": null
}`;

export const VECTOR_DIGEST = "0xfe0bcc7d9e69ee02881011f29a4156caa15e02b8e94c2e0c9f40e66711651993";
export const VECTOR_NONCE = "0x18b79105e486e10f626b71939a0226c47316949b7c24ff1c397661ea861fafaa";

/** The signer's checklist, shared by the page and the prompt. */
export const CHECKLIST = [
  "typed_data.message.from is your Payday wallet (the withdrawal's wallet_address).",
  "typed_data.domain.chainId is the leg's source_chain.id, and verifyingContract is that chain's native USDC.",
  "typed_data.message.value equals the leg's amount_base_units, and validAfter is \"0\".",
  "For a transfer leg, typed_data.message.to is your destination address.",
  "For a bridge leg, typed_data.message.to is the WithdrawalForwarder the docs list for that chain (also in authorization.forwarder), nonce_preimage.mint_recipient is your destination address, and typed_data.message.nonce equals keccak256(abi.encode(uint32 destination_domain, bytes32(mint_recipient), bytes32 salt)) over nonce_preimage.",
];

export const WITHDRAWALS_PROMPT = `Build the Payday withdrawal flow for this server. Payday (https://payday.sh) holds our USDC in a "Payday wallet" on Monad (chain 143), Base (8453) and Arbitrum One (42161). A withdrawal moves the wallet's whole USDC balance on every network to one address we name, on one of those networks, with nothing deducted: Payday relays and pays gas, and our signature decides where each leg's funds may land. Read https://payday.sh/docs/withdrawals and https://payday.sh/docs/api/withdrawals/create first.

## Flow: prepare, sign, submit, poll

1. POST https://api.payday.sh/v1/withdrawals with headers Authorization: Bearer $PAYDAY_API_KEY, Content-Type: application/json, Idempotency-Key: <a fresh UUID>, and body {"destination": {"chain_id": "<decimal chain id>", "address": "<0x address we control on that chain>"}}. Response 201: a withdrawal with status "awaiting_signature" and one leg per network the wallet holds USDC on. Each leg has kind "transfer" (funds already on the destination chain) or "bridge" (moved through Circle's CCTP), amount_base_units (6 decimals), and authorization.typed_data: an EIP-712 document under that chain's USDC contract, whose primaryType is "TransferWithAuthorization" (transfer leg) or "ReceiveWithAuthorization" (bridge leg). Every uint256 in the document is a decimal string; nonce is 0x-hex bytes32; domain.chainId is a JSON number.
   Errors: 400 invalid_request; 409 wallet_not_ready, withdrawal_in_progress (one open withdrawal per account; cancel or finish it), nothing_to_withdraw, idempotency_conflict; 503 withdrawals_unavailable.
2. For every leg with state "awaiting_signature", verify the document, then sign it with eth_signTypedData_v4 semantics (EIP-712: keccak256(0x1901 || domainSeparator || hashStruct(message))) using the Payday wallet's private key, exported once from the Payday dashboard (Account section, "Export wallet key") and kept in a secret manager. The signature is 65 bytes r || s || v with v = 27 or 28, as 0x-prefixed hex.
   Verification before signing, refuse otherwise:
${CHECKLIST.map((item) => `   - ${item}`).join("\n")}
   The domain differs per chain and is read from the token: name "USDC" on Monad, "USD Coin" on Base and Arbitrum, version "2". Use the domain the API sent; do not hardcode it.
3. POST /v1/withdrawals/{id}/authorizations with {"authorizations": [{"leg_id": "...", "signature": "0x..."}]} (any subset of legs; all signatures are verified before any is stored). Response 200: the withdrawal, status "in_progress" once every leg is signed. Errors: 400 signature_invalid (message names the leg); 404 withdrawal_not_found, withdrawal_leg_not_found; 409 leg_not_awaiting_signature, authorization_expired (24 hours after creation; create a new withdrawal), withdrawal_finished.
4. GET /v1/withdrawals/{id} every 10 seconds until status is "completed", "failed" or "cancelled". Leg states: awaiting_signature, authorized, relaying, burned, attested, minting, completed, failed, expired, cancelled. A bridge leg's burn is attested by Circle in seconds from Monad and in about 15-20 minutes from Base or Arbitrum; then Payday mints on the destination chain. Each leg reports transfer_tx_hash, burn_tx_hash, mint_tx_hash and failure_reason. POST /v1/withdrawals/{id}/cancel works until a leg has been relayed (409 withdrawal_not_cancellable afterwards). GET /v1/withdrawals lists withdrawals newest first ({"withdrawals": [...], "next_cursor": "wd_..." | null}, limit 1-100, starting_after).

## Test vector (all implementations agree on it)

Domain {name "USDC", version "2", chainId 143, verifyingContract 0x754704Bc059F8C67012fEd69BC8A327a5aafb603}; ReceiveWithAuthorization {from 0x1111111111111111111111111111111111111111, to 0x2222222222222222222222222222222222222222, value 1234567, validAfter 0, validBefore 1800000000, nonce ${VECTOR_NONCE}} hashes to the EIP-712 signing digest ${VECTOR_DIGEST}. That nonce is keccak256(abi.encode(uint32 6, bytes32(0x000000000000000000000000000000000000d00d), bytes32(0x…07))). A full leg as the API returns it:

${LEG_EXAMPLE}

## Reference implementations

### TypeScript (the official SDK, @payday/sdk, with @payday/sdk/signing and viem)

${WITHDRAW_TS}

### Rust (alloy + reqwest)

${WITHDRAW_RUST}

### Go (go-ethereum)

${WITHDRAW_GO}

Implement the same four steps in this server's language with its usual HTTP client and an EIP-712 library that takes the document as JSON (decimal strings for uint256). Keep the wallet key out of source control, verify each document before signing, treat a 409 withdrawal_in_progress as "poll or cancel the existing one", and stop polling on a terminal status.`;
