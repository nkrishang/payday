#!/usr/bin/env node
// A stand-in for Relay (api.relay.link) for the local Anvil stack: enough of
// `/chains`, `/quote/v2`, `/intents/status/v3`, and `/requests/v3` for
// gatewayd to quote a cross-chain payment and for gateway-indexer to follow
// it, plus the solver's side, which Relay itself never exposes: once the
// payer's deposit is seen on the origin Anvil, the stub sends USDC from its
// own solver key to the recipient on the destination Anvil, exactly as a
// Relay solver fills a request.
//
// Origin chain: PAYDAY_SECOND_CHAIN_ID at PAYDAY_SECOND_RPC_URL (31338).
// Destination chain: PAYDAY_CHAIN_ID at PAYDAY_RPC_URL (31337). Both hold
// the same MockUSDC at RELAY_STUB_USDC; the solver key holds USDC on the
// destination chain (the suite funds its USDC; the stub gives it gas with anvil_setBalance) and is a fixed key outside Anvil's ten accounts, which the suite uses as beneficiaries.
//
// The quote's one step is a plain USDC `transfer` from the payer to the
// solver on the origin chain for the quoted input amount. A real quote is
// an approve and a router call; the page treats every step as an opaque
// transaction, so a transfer exercises the same path.
//
// Every call must carry `x-api-key`, as Relay requires from 2026-10-02.
//
// Environment: RELAY_STUB_PORT (4020), RELAY_STUB_API_KEY (local),
// RELAY_STUB_USDC, RELAY_STUB_SOLVER_KEY, PAYDAY_RPC_URL, PAYDAY_CHAIN_ID,
// PAYDAY_SECOND_RPC_URL, PAYDAY_SECOND_CHAIN_ID. Needs `cast` on the PATH.
//
// Control endpoints for the suite: `POST /__fail/{requestId}` makes the stub
// report `failure` for that request instead of filling it.

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createServer } from "node:http";

const PORT = Number(process.env.RELAY_STUB_PORT ?? 4020);
const API_KEY = process.env.RELAY_STUB_API_KEY ?? "local";
const USDC = (process.env.RELAY_STUB_USDC ?? "").toLowerCase();
const SOLVER_KEY =
  process.env.RELAY_STUB_SOLVER_KEY ??
  "0x1111111111111111111111111111111111111111111111111111111111111111";
const DESTINATION = {
  id: Number(process.env.PAYDAY_CHAIN_ID ?? 31337),
  rpc: process.env.PAYDAY_RPC_URL ?? "http://127.0.0.1:8545",
};
const ORIGIN = {
  id: Number(process.env.PAYDAY_SECOND_CHAIN_ID ?? 31338),
  rpc: process.env.PAYDAY_SECOND_RPC_URL ?? "http://127.0.0.1:8546",
};
/** What the solver charges on top of the exact output, in base units. */
const FEE = 20_000n;
if (!/^0x[0-9a-f]{40}$/.test(USDC)) {
  console.error("RELAY_STUB_USDC must be the local USDC address");
  process.exit(1);
}

const solver = cast(["wallet", "address", "--private-key", SOLVER_KEY]).trim();
const TRANSFER_TOPIC = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
// Gas for the fills: the solver is not one of Anvil's funded accounts.
rpc(DESTINATION.rpc, "anvil_setBalance", [solver, "0x3635c9adc5dea00000"]);

/** requestId → the quote and what became of it. */
const requests = new Map();

function cast(args, input) {
  return execFileSync("cast", args, { encoding: "utf8", input });
}

function rpc(url, method, params) {
  const out = cast(["rpc", "--rpc-url", url, method, ...params.map((param) => JSON.stringify(param))]);
  return JSON.parse(out);
}

function chainRecord(chain, name) {
  return {
    id: chain.id,
    name: name.toLowerCase(),
    displayName: name,
    httpRpcUrl: chain.rpc,
    explorerUrl: "",
    iconUrl: "",
    vmType: "evm",
    depositEnabled: true,
    disabled: false,
    currency: { symbol: "ETH", decimals: 18 },
    solverCurrencies: [{ id: "usdc", symbol: "USDC", address: USDC, decimals: 6 }],
  };
}

const CHAINS = {
  chains: [chainRecord(DESTINATION, "Local"), chainRecord(ORIGIN, "Local Two")],
};

function word32(address) {
  return `0x${"0".repeat(24)}${address.slice(2).toLowerCase()}`;
}

/** The payer's transfer to the solver on the origin chain, if it has landed. */
function findDeposit(request) {
  const logs = rpc(ORIGIN.rpc, "eth_getLogs", [
    {
      fromBlock: "0x0",
      toBlock: "latest",
      address: USDC,
      topics: [TRANSFER_TOPIC, word32(request.user), word32(solver)],
    },
  ]);
  for (const log of logs) {
    if (BigInt(log.data) === request.amountIn) {
      const tx = rpc(ORIGIN.rpc, "eth_getTransactionByHash", [log.transactionHash]);
      return { hash: log.transactionHash, from: tx?.from ?? request.user };
    }
  }
  return null;
}

/** The fill: USDC from the solver to the recipient on the destination chain. */
function fill(request) {
  const out = cast([
    "send",
    "--rpc-url",
    DESTINATION.rpc,
    "--private-key",
    SOLVER_KEY,
    "--json",
    USDC,
    "transfer(address,uint256)",
    request.recipient,
    request.amountOut.toString(),
  ]);
  return JSON.parse(out).transactionHash;
}

/** Advance the request one state per poll, the way the suite expects. */
function observe(request) {
  if (request.status === "failure" || request.status === "success") return;
  if (request.failNext) {
    request.status = "failure";
    return;
  }
  const deposit = findDeposit(request);
  if (!deposit) return;
  request.status = "success";
  request.inTx = deposit;
  request.outTx = fill(request);
}

function statusOf(request) {
  return {
    status: request.status,
    inTxHashes: request.inTx ? [request.inTx.hash] : [],
    txHashes: request.outTx ? [request.outTx] : [],
    originChainId: request.originChainId,
    destinationChainId: request.destinationChainId,
  };
}

function recordOf(request) {
  return {
    id: request.id,
    status: request.status,
    user: request.user,
    recipient: request.recipient,
    data: {
      inTxs: request.inTx ? [{ hash: request.inTx.hash, chainId: request.originChainId, status: "success" }] : [],
      outTxs: request.outTx ? [{ hash: request.outTx, chainId: request.destinationChainId, status: "success" }] : [],
    },
    protocol: {
      deposit: {
        origin: request.inTx
          ? { depositor: request.inTx.from, chainId: request.originChainId, transactionId: request.inTx.hash }
          : null,
      },
    },
  };
}

function send(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

async function readJson(req) {
  let body = "";
  for await (const chunk of req) body += chunk;
  return body ? JSON.parse(body) : {};
}

createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${PORT}`);
  try {
    if (url.pathname.startsWith("/__fail/")) {
      const request = requests.get(url.pathname.slice("/__fail/".length).toLowerCase());
      if (!request) return send(res, 404, { message: "unknown request" });
      request.failNext = true;
      return send(res, 200, { ok: true });
    }
    if (req.headers["x-api-key"] !== API_KEY) {
      return send(res, 401, { message: "Please provide an api key", errorCode: "UNAUTHORIZED_QUOTE" });
    }
    if (req.method === "GET" && url.pathname === "/chains") return send(res, 200, CHAINS);
    if (req.method === "POST" && url.pathname === "/quote/v2") {
      const body = await readJson(req);
      const originChainId = Number(body.originChainId);
      const destinationChainId = Number(body.destinationChainId);
      if (originChainId !== ORIGIN.id || destinationChainId !== DESTINATION.id) {
        return send(res, 400, { message: "No solver for this route", errorCode: "NO_QUOTES" });
      }
      if (body.tradeType !== "EXACT_OUTPUT") {
        return send(res, 400, { message: "only EXACT_OUTPUT is supported here", errorCode: "UNSUPPORTED" });
      }
      const amountOut = BigInt(body.amount);
      const amountIn = amountOut + FEE;
      const id = `0x${randomBytes(32).toString("hex")}`;
      const request = {
        id,
        status: "waiting",
        user: String(body.user).toLowerCase(),
        recipient: String(body.recipient),
        originChainId,
        destinationChainId,
        amountIn,
        amountOut,
        inTx: null,
        outTx: null,
        failNext: false,
      };
      requests.set(id, request);
      const calldata = `0xa9059cbb${word32(solver).slice(2)}${amountIn.toString(16).padStart(64, "0")}`;
      return send(res, 200, {
        requestId: id,
        steps: [
          {
            id: "deposit",
            action: "Confirm transaction in your wallet",
            kind: "transaction",
            requestId: id,
            items: [
              {
                status: "incomplete",
                data: { from: body.user, to: USDC, data: calldata, value: "0", chainId: originChainId, gas: "80000" },
                check: { endpoint: `/intents/status/v3?requestId=${id}`, method: "GET" },
              },
            ],
          },
        ],
        fees: { relayer: { amountUsd: "0.02" } },
        details: {
          operation: "bridge",
          currencyIn: { currency: { chainId: originChainId }, amount: amountIn.toString() },
          currencyOut: { currency: { chainId: destinationChainId }, amount: amountOut.toString() },
          timeEstimate: 2,
        },
      });
    }
    if (req.method === "GET" && url.pathname === "/intents/status/v3") {
      const request = requests.get(String(url.searchParams.get("requestId") ?? "").toLowerCase());
      if (!request) return send(res, 200, { status: "unknown" });
      observe(request);
      return send(res, 200, statusOf(request));
    }
    if (req.method === "GET" && url.pathname === "/requests/v3") {
      const request = requests.get(String(url.searchParams.get("id") ?? "").toLowerCase());
      if (!request) return send(res, 200, { requests: [] });
      observe(request);
      return send(res, 200, { requests: [recordOf(request)] });
    }
    return send(res, 404, { message: "not found" });
  } catch (error) {
    console.error(`relay stub: ${error?.message ?? error}`);
    return send(res, 500, { message: String(error?.message ?? error) });
  }
}).listen(PORT, "127.0.0.1", () => {
  console.log(`relay stub on http://127.0.0.1:${PORT}, solver ${solver}`);
});
