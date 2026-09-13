//! Relay (relay.link) for cross-chain payments into a deposit address.
//!
//! The hosted checkout lets a payer whose USDC sits on another chain pay a
//! deposit request anyway. `gatewayd` asks Relay for an `EXACT_OUTPUT`
//! quote whose `user` is the attested wallet, whose `recipient` is the
//! payment address, and whose destination currency is USDC on the request's
//! chain; the page sends the quote's transactions from that wallet; Relay's
//! solver delivers exactly the quoted amount to the payment address, in
//! seconds. `gateway-indexer` then follows the request until it is filled
//! and attributes the solver's transfer to the payer.
//!
//! Every call carries the API key: from 2026-10-02 Relay requires one on
//! every quote, and `GET /requests/v3` already does. The key never reaches
//! the browser, which is why the quote is made here and not in the page.
//!
//! Endpoints used, all under `https://api.relay.link`:
//!
//! - `GET /chains`: the chains Relay serves, with each one's solver
//!   currencies (the assets a payer may deposit).
//! - `POST /quote/v2`: the quote; its `steps[].items[].data` are the
//!   transactions to send, in order (an ERC-20 `approve`, then the deposit).
//! - `GET /intents/status/v3?requestId=`: the request's state and the fill's
//!   destination transaction hashes.
//! - `GET /requests/v3?id=`: the request record, which names the origin
//!   depositor. It is what lets the attribution say who spent the funds.

use alloy_primitives::{Address, B256, Bytes, U256, hex};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

pub const DEFAULT_RELAY_URL: &str = "https://api.relay.link";
/// The `referrer` Relay attributes our quotes to.
pub const REFERRER: &str = "payday";

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("relay request failed: {0}")]
    Transport(String),
    #[error("relay answered {status}: {message}")]
    Status {
        status: u16,
        code: Option<String>,
        message: String,
    },
    #[error("relay answer was malformed: {0}")]
    Malformed(String),
}

/// One chain Relay serves, as `GET /chains` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayChain {
    pub id: u64,
    /// Relay's slug (`base`), for logs.
    pub name: String,
    /// The name to show (`Base`).
    pub display_name: String,
    pub explorer_url: Option<String>,
    pub icon_url: Option<String>,
    /// A public RPC for the chain, so a wallet can be asked to add it.
    pub http_rpc_url: Option<String>,
    pub vm_type: String,
    pub deposit_enabled: bool,
    pub disabled: bool,
    /// The gas token's symbol.
    pub native_symbol: Option<String>,
    /// The assets a payer may deposit on this chain.
    pub solver_currencies: Vec<RelayCurrency>,
}

impl RelayChain {
    /// Whether a payer may pay from this chain with USDC: an EVM chain
    /// taking deposits whose solvers hold USDC.
    pub fn usdc(&self) -> Option<&RelayCurrency> {
        if self.vm_type != "evm" || !self.deposit_enabled || self.disabled {
            return None;
        }
        self.solver_currencies
            .iter()
            .find(|currency| currency.symbol == "USDC")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCurrency {
    pub symbol: String,
    pub address: Address,
    pub decimals: u8,
}

/// What a quote asks for. Always `EXACT_OUTPUT` with `enableTrueExactOutput`:
/// the payer pays whatever the route costs and exactly `amount` lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteRequest {
    /// The wallet that will send the origin transactions: the attested one.
    pub user: Address,
    /// The payment address on the destination chain.
    pub recipient: Address,
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub origin_currency: Address,
    pub destination_currency: Address,
    /// Base units of the destination currency.
    pub amount: U256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub request_id: B256,
    /// Base units of the origin currency the payer sends.
    pub amount_in: U256,
    /// Base units of the destination currency that land.
    pub amount_out: U256,
    /// Relay's fee in USD, as a decimal string, when it reports one.
    pub relayer_fee_usd: Option<String>,
    pub time_estimate_secs: u64,
    pub steps: Vec<QuoteStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteStep {
    /// `approve`, `deposit`, ...
    pub id: String,
    /// `transaction` or `signature`; only transactions are supported.
    pub kind: String,
    pub transactions: Vec<StepTransaction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepTransaction {
    pub chain_id: u64,
    pub from: Address,
    pub to: Address,
    pub data: Bytes,
    pub value: U256,
    pub gas: Option<u64>,
}

/// `GET /intents/status/v3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentStatus {
    pub state: IntentState,
    /// Relay's own word, for the record.
    pub raw: String,
    /// The origin transactions Relay saw.
    pub in_tx_hashes: Vec<B256>,
    /// The destination transactions that filled the request.
    pub tx_hashes: Vec<B256>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentState {
    /// No deposit yet, or Relay does not know the request (`unknown`).
    Waiting,
    /// Deposited, filling, or delayed: not done, not failed.
    Pending,
    Success,
    Refund,
    Failure,
}

impl IntentState {
    pub fn parse(status: &str) -> Self {
        match status {
            "success" => Self::Success,
            "refund" => Self::Refund,
            "failure" => Self::Failure,
            "waiting" | "unknown" => Self::Waiting,
            _ => Self::Pending,
        }
    }
}

/// `GET /requests/v3?id=`: the parts of the record attribution needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayRequest {
    pub status: String,
    /// The origin sender, as Relay indexed the deposit.
    pub depositor: Option<Address>,
    pub in_txs: Vec<(u64, B256)>,
    pub out_txs: Vec<(u64, B256)>,
}

/// The seam the services go through, so they are testable without Relay.
/// `gatewayd` quotes; `gateway-indexer` follows.
#[async_trait]
pub trait RelayApi: Send + Sync {
    async fn chains(&self) -> Result<Vec<RelayChain>, RelayError>;
    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, RelayError>;
    async fn status(&self, request_id: B256) -> Result<IntentStatus, RelayError>;
    async fn request(&self, request_id: B256) -> Result<Option<RelayRequest>, RelayError>;
}

pub struct RelayClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl RelayClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, RelayError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|error| RelayError::Transport(error.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            http,
        })
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, RelayError> {
        let response = request
            .header("x-api-key", &self.api_key)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|error| RelayError::Transport(error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| RelayError::Transport(error.to_string()))?;
        if !status.is_success() {
            let (code, message) = error_body(&body);
            return Err(RelayError::Status {
                status: status.as_u16(),
                code,
                message,
            });
        }
        Ok(body)
    }
}

#[async_trait]
impl RelayApi for RelayClient {
    async fn chains(&self) -> Result<Vec<RelayChain>, RelayError> {
        let body = self
            .send(self.http.get(format!("{}/chains", self.base_url)))
            .await?;
        parse_chains(&body)
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, RelayError> {
        let body = serde_json::json!({
            "user": request.user.to_checksum(None),
            "recipient": request.recipient.to_checksum(None),
            "refundTo": request.user.to_checksum(None),
            "originChainId": request.origin_chain_id,
            "destinationChainId": request.destination_chain_id,
            "originCurrency": request.origin_currency.to_checksum(None),
            "destinationCurrency": request.destination_currency.to_checksum(None),
            "amount": request.amount.to_string(),
            "tradeType": "EXACT_OUTPUT",
            "enableTrueExactOutput": true,
            "referrer": REFERRER,
        });
        let answer = self
            .send(
                self.http
                    .post(format!("{}/quote/v2", self.base_url))
                    .json(&body),
            )
            .await?;
        parse_quote(&answer)
    }

    async fn status(&self, request_id: B256) -> Result<IntentStatus, RelayError> {
        let body = self
            .send(
                self.http
                    .get(format!("{}/intents/status/v3", self.base_url))
                    .query(&[("requestId", request_id.to_string())]),
            )
            .await?;
        parse_status(&body)
    }

    async fn request(&self, request_id: B256) -> Result<Option<RelayRequest>, RelayError> {
        let body = self
            .send(
                self.http
                    .get(format!("{}/requests/v3", self.base_url))
                    .query(&[("id", request_id.to_string())]),
            )
            .await?;
        parse_request(&body)
    }
}

/// Relay's error envelope: `{"message": ..., "errorCode": ...}` or a
/// Fastify validation error `{"code": ..., "message": ...}`.
fn error_body(body: &str) -> (Option<String>, String) {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let code = value
                .get("errorCode")
                .or_else(|| value.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| body.chars().take(200).collect());
            (code, message)
        }
        Err(_) => (None, body.chars().take(200).collect()),
    }
}

#[derive(Deserialize)]
struct ChainsResponse {
    chains: Vec<ChainRecord>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainRecord {
    id: u64,
    name: String,
    display_name: Option<String>,
    explorer_url: Option<String>,
    icon_url: Option<String>,
    http_rpc_url: Option<String>,
    vm_type: Option<String>,
    #[serde(default)]
    deposit_enabled: bool,
    #[serde(default)]
    disabled: bool,
    currency: Option<CurrencyRecord>,
    #[serde(default)]
    solver_currencies: Vec<CurrencyRecord>,
}

#[derive(Deserialize)]
struct CurrencyRecord {
    symbol: String,
    address: Option<String>,
    decimals: Option<u8>,
}

pub fn parse_chains(body: &str) -> Result<Vec<RelayChain>, RelayError> {
    let parsed: ChainsResponse =
        serde_json::from_str(body).map_err(|error| RelayError::Malformed(error.to_string()))?;
    let mut chains = Vec::with_capacity(parsed.chains.len());
    for record in parsed.chains {
        let solver_currencies = record
            .solver_currencies
            .into_iter()
            .filter_map(|currency| {
                let address = currency.address?.parse::<Address>().ok()?;
                Some(RelayCurrency {
                    symbol: currency.symbol,
                    address,
                    decimals: currency.decimals?,
                })
            })
            .collect();
        chains.push(RelayChain {
            id: record.id,
            display_name: record.display_name.unwrap_or_else(|| record.name.clone()),
            name: record.name,
            explorer_url: record.explorer_url.filter(|url| !url.is_empty()),
            icon_url: record.icon_url.filter(|url| !url.is_empty()),
            http_rpc_url: record.http_rpc_url.filter(|url| !url.is_empty()),
            vm_type: record.vm_type.unwrap_or_default(),
            deposit_enabled: record.deposit_enabled,
            disabled: record.disabled,
            native_symbol: record.currency.map(|currency| currency.symbol),
            solver_currencies,
        });
    }
    Ok(chains)
}

fn word(field: &str, value: &Value) -> Result<B256, RelayError> {
    value
        .as_str()
        .and_then(|text| text.parse::<B256>().ok())
        .ok_or_else(|| RelayError::Malformed(format!("{field} is not a 32-byte hex word")))
}

fn address(field: &str, value: &Value) -> Result<Address, RelayError> {
    value
        .as_str()
        .and_then(|text| text.parse::<Address>().ok())
        .ok_or_else(|| RelayError::Malformed(format!("{field} is not an address")))
}

fn uint(field: &str, value: &Value) -> Result<U256, RelayError> {
    match value {
        Value::String(text) => U256::from_str_radix(text, 10).ok(),
        Value::Number(number) => number.as_u64().map(U256::from),
        _ => None,
    }
    .ok_or_else(|| RelayError::Malformed(format!("{field} is not a decimal amount")))
}

fn u64_of(field: &str, value: &Value) -> Result<u64, RelayError> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
    .ok_or_else(|| RelayError::Malformed(format!("{field} is not an integer")))
}

pub fn parse_quote(body: &str) -> Result<Quote, RelayError> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| RelayError::Malformed(error.to_string()))?;
    let request_id = word("requestId", &value["requestId"])?;
    let details = &value["details"];
    let amount_in = uint(
        "details.currencyIn.amount",
        &details["currencyIn"]["amount"],
    )?;
    let amount_out = uint(
        "details.currencyOut.amount",
        &details["currencyOut"]["amount"],
    )?;
    let time_estimate_secs = details
        .get("timeEstimate")
        .map(|estimate| u64_of("details.timeEstimate", estimate))
        .transpose()?
        .unwrap_or(0);
    let relayer_fee_usd = value["fees"]["relayer"]["amountUsd"]
        .as_str()
        .map(str::to_owned);
    let steps = value["steps"]
        .as_array()
        .ok_or_else(|| RelayError::Malformed("steps is not an array".into()))?;
    let mut parsed_steps = Vec::with_capacity(steps.len());
    for step in steps {
        let id = step["id"]
            .as_str()
            .ok_or_else(|| RelayError::Malformed("step without id".into()))?
            .to_owned();
        let kind = step["kind"]
            .as_str()
            .ok_or_else(|| RelayError::Malformed("step without kind".into()))?
            .to_owned();
        let items = step["items"]
            .as_array()
            .ok_or_else(|| RelayError::Malformed("step items is not an array".into()))?;
        let mut transactions = Vec::with_capacity(items.len());
        for item in items {
            let data = &item["data"];
            let calldata = data["data"]
                .as_str()
                .map(|text| hex::decode(text).map(Bytes::from))
                .transpose()
                .map_err(|_| RelayError::Malformed("step data is not hex".into()))?
                .unwrap_or_default();
            transactions.push(StepTransaction {
                chain_id: u64_of("step chainId", &data["chainId"])?,
                from: address("step from", &data["from"])?,
                to: address("step to", &data["to"])?,
                data: calldata,
                value: match data.get("value") {
                    Some(value) if !value.is_null() => uint("step value", value)?,
                    _ => U256::ZERO,
                },
                gas: data
                    .get("gas")
                    .filter(|gas| !gas.is_null())
                    .map(|gas| u64_of("step gas", gas))
                    .transpose()?,
            });
        }
        parsed_steps.push(QuoteStep {
            id,
            kind,
            transactions,
        });
    }
    Ok(Quote {
        request_id,
        amount_in,
        amount_out,
        relayer_fee_usd,
        time_estimate_secs,
        steps: parsed_steps,
    })
}

pub fn parse_status(body: &str) -> Result<IntentStatus, RelayError> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| RelayError::Malformed(error.to_string()))?;
    let raw = value["status"]
        .as_str()
        .ok_or_else(|| RelayError::Malformed("status missing".into()))?
        .to_owned();
    let hashes = |field: &str| -> Result<Vec<B256>, RelayError> {
        value[field]
            .as_array()
            .map(|hashes| hashes.iter().map(|hash| word(field, hash)).collect())
            .unwrap_or_else(|| Ok(Vec::new()))
    };
    Ok(IntentStatus {
        state: IntentState::parse(&raw),
        in_tx_hashes: hashes("inTxHashes")?,
        tx_hashes: hashes("txHashes")?,
        raw,
    })
}

pub fn parse_request(body: &str) -> Result<Option<RelayRequest>, RelayError> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| RelayError::Malformed(error.to_string()))?;
    let Some(record) = value["requests"]
        .as_array()
        .and_then(|requests| requests.first())
    else {
        return Ok(None);
    };
    let status = record["status"].as_str().unwrap_or("").to_owned();
    let depositor = record["protocol"]["deposit"]["origin"]["depositor"]
        .as_str()
        .or_else(|| record["user"].as_str())
        .and_then(|text| text.parse::<Address>().ok());
    let txs = |field: &str| -> Result<Vec<(u64, B256)>, RelayError> {
        record["data"][field]
            .as_array()
            .map(|txs| {
                txs.iter()
                    .filter(|tx| {
                        tx["status"]
                            .as_str()
                            .is_none_or(|status| status == "success")
                    })
                    .map(|tx| {
                        Ok((
                            u64_of("chainId", &tx["chainId"])?,
                            word("hash", &tx["hash"])?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_else(|| Ok(Vec::new()))
    };
    Ok(Some(RelayRequest {
        status,
        depositor,
        in_txs: txs("inTxs")?,
        out_txs: txs("outTxs")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `POST /quote/v2` answer captured on 2026-09-13: 5 USDC to land on
    /// Monad, paid in USDC from Base, trimmed to the fields we read.
    const QUOTE: &str = r#"{
      "requestId": "0x1789270142d0cafb3e82a8bce00a1efd499f233705cef1284b3688cd205cc788",
      "steps": [
        {"id": "approve", "kind": "transaction", "items": [{"status": "incomplete", "data": {
          "from": "0x000000000000000000000000000000000000dEaD",
          "to": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
          "data": "0x095ea7b30000000000000000000000004cd00e387622c35bddb9b4c962c136462338bc3100000000000000000000000000000000000000000000000000000000004c9c7e",
          "value": "0", "chainId": 8453, "gas": "73112", "maxFeePerGas": "16268390"}}]},
        {"id": "deposit", "kind": "transaction", "items": [{"status": "incomplete",
          "data": {"from": "0x000000000000000000000000000000000000dEaD",
          "to": "0x4cd00e387622c35bddb9b4c962c136462338bc31",
          "data": "0xe8017952", "value": "0", "chainId": 8453},
          "check": {"endpoint": "/intents/status/v3?requestId=0x17", "method": "GET"}}]}
      ],
      "fees": {"gas": {"amountUsd": "0.003116"}, "relayer": {"amountUsd": "0.020795"}},
      "details": {
        "operation": "swap",
        "currencyIn": {"currency": {"chainId": 8453}, "amount": "5020798", "amountFormatted": "5.020798"},
        "currencyOut": {"currency": {"chainId": 143}, "amount": "5000000", "amountFormatted": "5.0"},
        "timeEstimate": 1
      }
    }"#;

    #[test]
    fn quote_yields_the_transactions_in_order_and_the_exact_output() {
        let quote = parse_quote(QUOTE).unwrap();
        assert_eq!(
            quote.request_id.to_string(),
            "0x1789270142d0cafb3e82a8bce00a1efd499f233705cef1284b3688cd205cc788"
        );
        assert_eq!(quote.amount_in, U256::from(5_020_798u64));
        assert_eq!(quote.amount_out, U256::from(5_000_000u64));
        assert_eq!(quote.relayer_fee_usd.as_deref(), Some("0.020795"));
        assert_eq!(quote.time_estimate_secs, 1);
        assert_eq!(
            quote
                .steps
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            ["approve", "deposit"]
        );
        let approve = &quote.steps[0].transactions[0];
        assert_eq!(approve.chain_id, 8453);
        assert_eq!(approve.gas, Some(73_112));
        assert_eq!(approve.data.len(), 68);
        let deposit = &quote.steps[1].transactions[0];
        assert_eq!(
            deposit.to,
            "0x4cd00e387622c35bddb9b4c962c136462338bc31"
                .parse::<Address>()
                .unwrap()
        );
        assert_eq!(deposit.gas, None);
        assert_eq!(deposit.value, U256::ZERO);
    }

    #[test]
    fn quote_without_a_request_id_is_malformed() {
        assert!(matches!(
            parse_quote(r#"{"steps": []}"#),
            Err(RelayError::Malformed(_))
        ));
    }

    #[test]
    fn status_maps_relays_vocabulary_onto_ours() {
        let success = parse_status(r#"{"status":"success","inTxHashes":["0xd988132f8d94619ad1d94fa70215dec16dca069db50cbe67ce8ba193e377f19f"],"txHashes":["0x5f1778ef72420fa2d04538b539c4ff55f12a49d34268c3086652af2270cace9f"],"originChainId":8453,"destinationChainId":143}"#).unwrap();
        assert_eq!(success.state, IntentState::Success);
        assert_eq!(success.tx_hashes.len(), 1);
        assert_eq!(success.in_tx_hashes.len(), 1);
        let unknown = parse_status(r#"{"status":"unknown"}"#).unwrap();
        assert_eq!(unknown.state, IntentState::Waiting);
        assert!(unknown.tx_hashes.is_empty());
        for (raw, state) in [
            ("waiting", IntentState::Waiting),
            ("depositing", IntentState::Pending),
            ("pending", IntentState::Pending),
            ("submitted", IntentState::Pending),
            ("delayed", IntentState::Pending),
            ("refund", IntentState::Refund),
            ("failure", IntentState::Failure),
        ] {
            assert_eq!(IntentState::parse(raw), state, "{raw}");
        }
    }

    #[test]
    fn request_names_the_depositor_and_the_fill() {
        let body = r#"{"requests":[{"id":"0x17","status":"success","user":"0x1884c66fd0eb29e356e6b5d455048bca46bb690f","recipient":"0x1884c66fd0eb29e356e6b5d455048bca46bb690f","data":{"inTxs":[{"hash":"0xd988132f8d94619ad1d94fa70215dec16dca069db50cbe67ce8ba193e377f19f","chainId":8453,"status":"success"}],"outTxs":[{"hash":"0x5f1778ef72420fa2d04538b539c4ff55f12a49d34268c3086652af2270cace9f","chainId":143,"status":"success"},{"hash":"0x5f1778ef72420fa2d04538b539c4ff55f12a49d34268c3086652af2270cace9e","chainId":143,"status":"pending"}]},"protocol":{"deposit":{"origin":{"depositor":"0x1884c66fd0eb29e356e6b5d455048bca46bb690f","chainId":8453}}}}]}"#;
        let request = parse_request(body).unwrap().unwrap();
        assert_eq!(request.status, "success");
        assert_eq!(
            request.depositor,
            Some(
                "0x1884c66fd0eb29e356e6b5d455048bca46bb690f"
                    .parse()
                    .unwrap()
            )
        );
        assert_eq!(request.in_txs.len(), 1);
        assert_eq!(request.in_txs[0].0, 8453);
        // Only successful destination transactions count as the fill.
        assert_eq!(request.out_txs.len(), 1);
        assert_eq!(request.out_txs[0].0, 143);
        assert_eq!(parse_request(r#"{"requests":[]}"#).unwrap(), None);
    }

    #[test]
    fn chains_expose_usdc_where_a_payer_may_deposit_it() {
        let body = r#"{"chains":[
          {"id":143,"name":"monad","displayName":"Monad","explorerUrl":"https://monadvision.com","iconUrl":"https://assets.relay.link/icons/143/light.png","httpRpcUrl":"https://rpc.monad.xyz","vmType":"evm","depositEnabled":true,"disabled":false,"currency":{"symbol":"MON","decimals":18},"solverCurrencies":[{"id":"usdc","symbol":"USDC","address":"0x754704bc059f8c67012fed69bc8a327a5aafb603","decimals":6}]},
          {"id":792703809,"name":"solana","displayName":"Solana","vmType":"svm","depositEnabled":true,"disabled":false,"solverCurrencies":[{"symbol":"USDC","address":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","decimals":6}]},
          {"id":1,"name":"ethereum","displayName":"Ethereum","vmType":"evm","depositEnabled":false,"disabled":false,"solverCurrencies":[{"symbol":"USDC","address":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","decimals":6}]}
        ]}"#;
        let chains = parse_chains(body).unwrap();
        assert_eq!(chains.len(), 3);
        let monad = &chains[0];
        assert_eq!(monad.display_name, "Monad");
        assert_eq!(monad.native_symbol.as_deref(), Some("MON"));
        assert_eq!(monad.usdc().unwrap().decimals, 6);
        // A non-EVM chain's USDC has no EVM address; a chain not taking
        // deposits offers nothing.
        assert!(chains[1].usdc().is_none());
        assert!(chains[2].usdc().is_none());
    }

    #[test]
    fn errors_carry_relays_message() {
        assert_eq!(
            error_body(
                r#"{"message":"Please provide an api key","errorCode":"UNAUTHORIZED_QUOTE"}"#
            ),
            (
                Some("UNAUTHORIZED_QUOTE".into()),
                "Please provide an api key".into()
            )
        );
        assert_eq!(
            error_body(r#"{"statusCode":400,"code":"FST_ERR_VALIDATION","message":"headers must have required property 'x-api-key'"}"#).0,
            Some("FST_ERR_VALIDATION".into())
        );
        assert_eq!(error_body("<html>").1, "<html>");
    }
}
