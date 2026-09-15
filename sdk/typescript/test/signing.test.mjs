import assert from "node:assert/strict";
import test from "node:test";
import { encodeAbiParameters, hashTypedData, keccak256, recoverTypedDataAddress } from "viem";
import { PaydayClient } from "../dist/index.js";
import {
  assertLegAuthorization,
  privateKeySigner,
  signWithdrawal,
  toSignableTypedData,
} from "../dist/signing.js";

// The vector gateway-core, the forwarder's Solidity test, and the web adapter
// pin: Monad's USDC domain, a bridge leg for 1.234567 USDC to 0x…d00d on
// Base (domain 6) with salt 0x…07.
const WALLET = "0x1111111111111111111111111111111111111111";
const FORWARDER = "0x2222222222222222222222222222222222222222";
const DESTINATION = "0x000000000000000000000000000000000000d00d";
const NONCE = "0x18b79105e486e10f626b71939a0226c47316949b7c24ff1c397661ea861fafaa";
const DIGEST = "0xfe0bcc7d9e69ee02881011f29a4156caa15e02b8e94c2e0c9f40e66711651993";
const chains = (id) => id === 143
  ? { tokens: { USDC: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", USDT: "0xe7cd86e13AC4309349F30B3435a9d337750fC82D" }, cctp: { domain: 15, forwarder: FORWARDER } }
  : id === 8453
    ? { tokens: { USDC: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" }, cctp: { domain: 6, forwarder: "0x3333333333333333333333333333333333333333" } }
    : null;
const signingOptions = { chains, keccak: { encodeAbiParameters, keccak256 } };

const bridgeLeg = {
  id: "wdl_1",
  kind: "bridge",
  source_chain: { id: "143", name: "Monad", native_symbol: "MON" },
  token: { symbol: "USDC", address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", decimals: 6 },
  amount: "1.234567",
  amount_base_units: "1234567",
  state: "awaiting_signature",
  authorization: {
    primary_type: "ReceiveWithAuthorization",
    typed_data: {
      domain: {
        name: "USDC",
        version: "2",
        chainId: 143,
        verifyingContract: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
      },
      primaryType: "ReceiveWithAuthorization",
      types: {
        EIP712Domain: [
          { name: "name", type: "string" },
          { name: "version", type: "string" },
          { name: "chainId", type: "uint256" },
          { name: "verifyingContract", type: "address" },
        ],
        ReceiveWithAuthorization: [
          { name: "from", type: "address" },
          { name: "to", type: "address" },
          { name: "value", type: "uint256" },
          { name: "validAfter", type: "uint256" },
          { name: "validBefore", type: "uint256" },
          { name: "nonce", type: "bytes32" },
        ],
      },
      message: {
        from: WALLET,
        to: FORWARDER,
        value: "1234567",
        validAfter: "0",
        validBefore: "1800000000",
        nonce: NONCE,
      },
    },
    expires_at: "2027-01-15T08:00:00Z",
    forwarder: FORWARDER,
    nonce_preimage: {
      destination_domain: 6,
      mint_recipient: DESTINATION,
      salt: "0x0000000000000000000000000000000000000000000000000000000000000007",
    },
  },
  transfer_tx_hash: null,
  burn_tx_hash: null,
  mint_tx_hash: null,
  failure_reason: null,
};

const withdrawal = {
  id: "wd_1",
  status: "awaiting_signature",
  wallet_address: WALLET,
  currency: "USDC",
  destination: { chain: { id: "8453", name: "Base", native_symbol: "ETH" }, address: DESTINATION },
  legs: [bridgeLeg],
  created_at: "2027-01-14T08:00:00Z",
  completed_at: null,
  cancelled_at: null,
  failed_at: null,
};

test("the typed data hashes to the digest every implementation pins", () => {
  assert.equal(hashTypedData(toSignableTypedData(bridgeLeg.authorization.typed_data)), DIGEST);
});

test("assertLegAuthorization refuses a document that strays from the leg", () => {
  assert.equal(assertLegAuthorization(withdrawal, bridgeLeg), bridgeLeg.authorization.typed_data);
  const tampered = (patch) => {
    const leg = structuredClone(bridgeLeg);
    patch(leg);
    return () => assertLegAuthorization(withdrawal, leg);
  };
  assert.throws(tampered((leg) => (leg.authorization.typed_data.message.to = DESTINATION)), /forwarder/);
  assert.throws(tampered((leg) => (leg.authorization.typed_data.message.value = "1")), /amount/);
  assert.throws(tampered((leg) => (leg.authorization.typed_data.message.from = FORWARDER)), /Payday wallet/);
  assert.throws(tampered((leg) => (leg.authorization.typed_data.domain.chainId = 8453)), /domain/);
  assert.throws(tampered((leg) => (leg.authorization.nonce_preimage.mint_recipient = FORWARDER)), /destination/);
  assert.throws(tampered((leg) => (leg.authorization.typed_data.message.validAfter = "1")), /validAfter/);
});

test("privateKeySigner signs every awaiting leg for the wallet and checks the nonce commitment", async () => {
  // Any key: the address it signs as must equal the withdrawal's wallet, so
  // the fixture wallet is replaced by the key's address here.
  const key = "0x0000000000000000000000000000000000000000000000000000000000000007";
  const signer = await privateKeySigner(key);
  const own = structuredClone(withdrawal);
  own.wallet_address = signer.address;
  own.legs[0].authorization.typed_data.message.from = signer.address;

  const { authorizations } = await signWithdrawal(own, signer, signingOptions);
  assert.equal(authorizations.length, 1);
  assert.equal(authorizations[0].leg_id, "wdl_1");
  const recovered = await recoverTypedDataAddress({
    ...toSignableTypedData(own.legs[0].authorization.typed_data),
    signature: authorizations[0].signature,
  });
  assert.equal(recovered, signer.address);

  const redirected = structuredClone(own);
  redirected.legs[0].authorization.nonce_preimage.salt = `0x${"08".padStart(64, "0")}`;
  await assert.rejects(
    signWithdrawal(redirected, signer, signingOptions),
    /does not commit to the documented destination/,
  );

  const signed = structuredClone(own);
  signed.legs[0].state = "authorized";
  signed.legs[0].authorization = null;
  assert.deepEqual(await signWithdrawal(signed, signer), { authorizations: [] });
});

test("bridge trust failures never reach the signer", async () => {
  const cases = [
    [(leg) => { leg.authorization.typed_data.message.nonce = `0x${"99".repeat(32)}`; }, /nonce/],
    [(leg) => { leg.authorization.forwarder = DESTINATION; leg.authorization.typed_data.message.to = DESTINATION; }, /forwarder/],
    [(leg) => { leg.authorization.nonce_preimage.destination_domain = 3; }, /destination domain/],
    [(leg) => { leg.authorization.typed_data.domain.verifyingContract = DESTINATION; }, /trusted USDC/],
    [(leg) => { leg.authorization.typed_data.message.validBefore = "1"; }, /expired/],
  ];
  for (const [tamper, message] of cases) {
    const bad = structuredClone(withdrawal);
    tamper(bad.legs[0]);
    let calls = 0;
    await assert.rejects(signWithdrawal(bad, { signTypedData: async () => { calls++; return "0xab"; } }, signingOptions), message);
    assert.equal(calls, 0);
  }
  let calls = 0;
  await assert.rejects(signWithdrawal(withdrawal, { signTypedData: async () => { calls++; return "0xab"; } }), /trusted chain registry/);
  assert.equal(calls, 0);
  await assert.rejects(
    signWithdrawal(withdrawal, { signTypedData: async () => { calls++; return "0xab"; } }, { ...signingOptions, chains: () => null }),
    /source chain/,
  );
  assert.equal(calls, 0);
  await assert.rejects(
    signWithdrawal(withdrawal, { signTypedData: async () => { calls++; return "0xab"; } }, {
      ...signingOptions,
      chains: (id) => id === 143 ? { ...chains(id), cctp: null } : chains(id),
    }),
    /no trusted CCTP/,
  );
  assert.equal(calls, 0);
});

test("one bad leg among several signs nothing", async () => {
  // The second leg is a valid document for a different amount; the first is
  // left honest. Nothing may be signed until every document has passed.
  const second = structuredClone(bridgeLeg);
  second.id = "wdl_2";
  second.amount = "2";
  second.amount_base_units = "2000000";
  second.authorization.typed_data.message.value = "2000000";
  const pair = { ...structuredClone(withdrawal), legs: [structuredClone(bridgeLeg), second] };

  let calls = 0;
  const signer = { signTypedData: async () => { calls++; return "0xab"; } };
  const { authorizations } = await signWithdrawal(pair, signer, signingOptions);
  assert.equal(authorizations.length, 2);

  const bad = structuredClone(pair);
  bad.legs[1].authorization.typed_data.message.nonce = `0x${"99".repeat(32)}`;
  await assert.rejects(signWithdrawal(bad, signer, signingOptions), /nonce/);
  assert.equal(calls, 2, "the honest pair signed, the tampered one signs nothing");
});

test("the client sends withdrawals the documented way", async () => {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, init });
    return new Response(JSON.stringify(withdrawal), { status: 201, headers: { "content-type": "application/json" } });
  };
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch });
  await client.withdrawals.create({ destination: { chain_id: "8453", address: DESTINATION } }, "w-1");
  assert.equal(calls[0].url, "https://example.test/v1/withdrawals");
  assert.equal(calls[0].init.headers["Idempotency-Key"], "w-1");
  await client.withdrawals.authorize("wd_1", [{ leg_id: "wdl_1", signature: "0xab" }]);
  assert.equal(calls[1].url, "https://example.test/v1/withdrawals/wd_1/authorizations");
  assert.deepEqual(JSON.parse(calls[1].init.body), { authorizations: [{ leg_id: "wdl_1", signature: "0xab" }] });
  await client.withdrawals.list({ limit: 5 });
  assert.equal(calls[2].url, "https://example.test/v1/withdrawals?limit=5");
  await client.withdrawals.cancel("wd_1");
  assert.equal(calls[3].url, "https://example.test/v1/withdrawals/wd_1/cancel");
  assert.equal(calls[3].init.method, "POST");
  assert.throws(() => client.withdrawals.create({ destination: { chain_id: "1", address: DESTINATION } }, ""), TypeError);
});
