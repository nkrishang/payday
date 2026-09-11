import { expect, test, type Page } from "@playwright/test";

/**
 * Every scenario is selected by the deposit request id; see e2e/stub-api.mjs.
 *
 * These cover what unit tests cannot: that the server-rendered page hydrates
 * without React discarding it, that polling moves the DOM on its own, and that
 * states which must not offer an address really do not.
 */

/**
 * Collects anything that would stop the page working: a hydration mismatch, a
 * thrown error, or a script the server refused. The last one is why this exists
 * — a dev-server origin check once answered 403 for every chunk, leaving HTML
 * that rendered correctly and never hydrated.
 */
function watchConsole(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("response", (response) => {
    if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`);
  });
  return errors;
}

const ADDRESS = "0x9a3f0000000000000000000000000000000000c2";
/** The wallet the stub's payers attest; the fake provider below signs as it. */
const PAYER_WALLET = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

async function expectNoInstructions(page: Page) {
  await expect(page.getByText(ADDRESS)).toHaveCount(0);
  await expect(page.getByRole("img", { name: /QR code/i })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /pay with wallet/i })).toHaveCount(0);
}

/**
 * A minimal EIP-1193 wallet at `window.ethereum`, which wagmi's injected
 * connector picks up: one account, on the configured chain, that signs any
 * typed data with a fixed signature. Enough to walk the wallet step without a
 * real extension; the stub API accepts any well-formed signature.
 */
async function installFakeWallet(page: Page) {
  await page.addInitScript(
    ({ wallet }) => {
      const listeners = new Map<string, Set<(...args: unknown[]) => void>>();
      const signed: unknown[] = [];
      const switched: string[] = [];
      // Starts on Monad; a switch request moves it, like a real wallet.
      let chainId = "0x8f";
      const provider = {
        isFakeWallet: true,
        request: async ({ method, params }: { method: string; params?: unknown[] }) => {
          switch (method) {
            case "eth_requestAccounts":
            case "eth_accounts":
              return [wallet];
            case "eth_chainId":
              return chainId;
            case "wallet_switchEthereumChain": {
              const [{ chainId: wanted }] = params as [{ chainId: string }];
              chainId = wanted;
              switched.push(wanted);
              for (const listener of listeners.get("chainChanged") ?? []) listener(wanted);
              return null;
            }
            case "eth_signTypedData_v4":
              signed.push(params);
              return `0x${"ab".repeat(64)}1b`;
            default:
              throw Object.assign(new Error(`unsupported ${method}`), { code: 4200 });
          }
        },
        on: (event: string, listener: (...args: unknown[]) => void) => {
          if (!listeners.has(event)) listeners.set(event, new Set());
          listeners.get(event)!.add(listener);
        },
        removeListener: (event: string, listener: (...args: unknown[]) => void) => {
          listeners.get(event)?.delete(listener);
        },
      };
      Object.assign(window, { ethereum: provider, __signed: signed, __switched: switched });
    },
    { wallet: PAYER_WALLET },
  );
}

test("an open deposit request shows everything needed to pay it", async ({ page }) => {
  const errors = watchConsole(page);
  await page.goto("/pay/dr_awaiting");

  await expect(page.getByText("Awaiting deposit")).toBeVisible();
  await expect(page.getByText("Amount due")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("25.00");
  await expect(page.getByText(ADDRESS)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByRole("button", { name: /pay with wallet/i })).toBeVisible();
  await expect(page.getByText(/Expires in/)).toBeVisible();

  // The exact contract, because a matching symbol is not enough.
  await expect(page.getByText("Monad · 143")).toBeVisible();
  await expect(page.getByText(/0x754704Bc/)).toBeVisible();

  expect(errors).toEqual([]);
});

test("the page hydrates and the countdown ticks", async ({ page }) => {
  const errors = watchConsole(page);
  await page.goto("/pay/dr_awaiting");

  const countdown = page.getByText(/Expires in/);
  const first = await countdown.textContent();
  await expect(async () => {
    expect(await countdown.textContent()).not.toBe(first);
  }).toPass({ timeout: 5_000 });

  // A ticking countdown means React took over the server-rendered markup.
  expect(errors).toEqual([]);
});

test("a partial deposit asks for the remainder and re-codes the QR", async ({ page }) => {
  await page.goto("/pay/dr_partial");

  await expect(page.getByText("Partially deposited")).toBeVisible();
  await expect(page.getByText("Remaining", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("15.00");
  await expect(page.getByText("10.00 of 25.00 USDC received")).toBeVisible();
  // The card is labelled with what is being asked for, for assistive tech.
  await expect(page.getByRole("region", { name: "Send the remaining 15.00 USDC" })).toBeVisible();
  await expect(page.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "40");
  // The QR is fetched again for the new remainder and shown from an object
  // URL: nothing about the deposit request or the session is in the image URL.
  await expect(page.getByRole("img", { name: /QR code to pay 15/i })).toHaveAttribute(
    "src",
    /^blob:/,
  );
});

test("polling moves the page without a reload", async ({ page, request }) => {
  await request.get("http://127.0.0.1:4010/__reset");
  await page.goto("/pay/dr_transition");

  await expect(page.getByText("Awaiting deposit")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("25.00");
  // The next read reports a partial deposit; nothing here reloads.
  await expect(page.getByText("Partially deposited")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("heading", { level: 1 })).toContainText("15.00");
});

test("a settled deposit request is a receipt, not an invitation to pay again", async ({ page }) => {
  await page.goto("/pay/dr_settled");

  await expect(page.getByText("Deposit complete")).toBeVisible();
  await expect(page.getByText("Exactly the requested amount reached the merchant.")).toBeVisible();
  // An exact deposit has nothing in recovery, so the receipt must not mention it.
  await expect(page.getByText(/recovery wallet/)).toHaveCount(0);
  await expect(page.getByRole("link", { name: /0x00210b33/ })).toBeVisible();
  await expectNoInstructions(page);
});

test("an overpaid settled deposit request says where the remainder went", async ({ page }) => {
  await page.goto("/pay/dr_settled-overpaid");

  await expect(page.getByText("Deposit complete")).toBeVisible();
  await expect(page.getByText(/Exactly the requested amount reached the merchant/)).toBeVisible();
  await expect(
    page.getByText(/above the requested amount went back to the wallet you signed with/),
  ).toBeVisible();
  // The receipt shows both what was asked for and what actually arrived.
  await expect(page.getByText("25.00 USDC")).toBeVisible();
  await expect(page.getByText("30.00 USDC")).toBeVisible();
  await expectNoInstructions(page);
});

test("a deposited request says settlement is still in progress", async ({ page }) => {
  await page.goto("/pay/dr_deposited");

  await expect(page.getByText("Deposit received")).toBeVisible();
  await expect(page.getByText(/settling the requested amount to the merchant/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an expired deposit request that received nothing asks for a new link", async ({ page }) => {
  await page.goto("/pay/dr_expired");

  await expect(page.getByText("This deposit link has expired")).toBeVisible();
  await expect(page.getByText(/Ask the merchant for a new deposit link/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an expired deposit request holding funds says where they went", async ({ page }) => {
  await page.goto("/pay/dr_expired-funded");

  await expect(page.getByText("The deadline passed before this deposit completed")).toBeVisible();
  await expect(page.getByText(/goes back to the wallet you signed with/)).toBeVisible();
  await expectNoInstructions(page);
});

test("a returned deposit request does the same", async ({ page }) => {
  await page.goto("/pay/dr_returned");

  await expect(page.getByText("This deposit was not completed in time")).toBeVisible();
  await expect(page.getByText(/back to the wallet you signed with/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an unbound request takes a network and the payer's signature before it shows any address", async ({
  page,
  request,
}) => {
  await installFakeWallet(page);
  const sessionInUrls: string[] = [];
  page.on("request", (sent) => {
    if (/pps_/.test(sent.url())) sessionInUrls.push(sent.url());
  });
  await page.goto("/pay/dr_unbound");

  // The content is open, the address is not: nothing invites a transfer yet.
  await expect(page.getByText("Wallet required")).toBeVisible();
  await expect(page.getByText("Amount due")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("25.00");
  await expect(page.getByText(/one-time payment destination for the network/i)).toBeVisible();
  await expectNoInstructions(page);
  const html = await (await request.get("/pay/dr_unbound")).text();
  expect(html).not.toContain(ADDRESS);

  // The network comes first: nothing can be signed until one is chosen.
  const networks = page.getByRole("radiogroup", { name: /network to pay on/i });
  await expect(networks.getByRole("radio", { name: /Monad/ })).toBeVisible();
  await expect(networks.getByRole("radio", { name: /Base/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /choose a network first/i })).toBeDisabled();
  await networks.getByRole("radio", { name: /Base/ }).click();
  await expect(networks.getByRole("radio", { name: /Base/ })).toHaveAttribute(
    "aria-checked",
    "true",
  );

  // Connect the fake wallet, then sign the challenge it is handed.
  await page.getByRole("button", { name: /connect the wallet you will pay from/i }).click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: /injected/i })
    .click();
  await expect(
    page.getByRole("button", { name: /sign to get your deposit address on Base/i }),
  ).toBeVisible();
  await page.getByRole("button", { name: /sign to get your deposit address on Base/i }).click();

  // The signature created the address on the chosen network; the page now
  // offers it, tied to the wallet that signed and to Base.
  await expect(page.getByText(ADDRESS)).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText(/Send from/)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByText("Base · 8453")).toBeVisible();
  await expect(page.getByText("Wallet required")).toHaveCount(0);
  // The wallet was switched to Base before signing, and what it was asked to
  // sign is the API's document, verbatim, under Base's domain.
  const switched = await page.evaluate(
    () => (window as unknown as { __switched: string[] }).__switched,
  );
  expect(switched).toContain("0x2105");
  const signed = await page.evaluate(() => (window as unknown as { __signed: unknown[] }).__signed);
  expect(signed).toHaveLength(1);
  const [signer, json] = signed[0] as [string, string];
  expect(signer.toLowerCase()).toBe(PAYER_WALLET.toLowerCase());
  const typed = JSON.parse(json);
  expect(typed.primaryType).toBe("PayerAttestation");
  expect(typed.domain.chainId).toBe(8453);
  expect(typed.message.wallet.toLowerCase()).toBe(PAYER_WALLET.toLowerCase());
  expect(typed.message.statement).toContain("Only transfers from this wallet");
  // The session never reached a URL.
  expect(sessionInUrls).toEqual([]);
});

test("a paused deposit request shows the gateway's own words", async ({ page }) => {
  await page.goto("/pay/dr_attention");

  await expect(page.getByText("Settlement is paused")).toBeVisible();
  await expect(page.getByText(/your funds remain safe/)).toBeVisible();
  await expect(page.getByText(/do not send a second transfer/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an address stops being offered the moment the gateway says it is not payable", async ({
  page,
}) => {
  await page.goto("/pay/dr_closing");

  await expect(page.getByText("The deadline has been reached")).toBeVisible();
  await expect(page.getByText(/Do not send funds now/)).toBeVisible();
  await expectNoInstructions(page);
});

test("the countdown reaching zero does not itself declare the deposit request expired", async ({
  page,
}) => {
  await page.goto("/pay/dr_ending");

  await expect(page.getByRole("button", { name: /pay with wallet/i })).toBeVisible();

  // Chain time decides expiry. When the deadline passes, the page stops
  // offering the address but must not claim an outcome the server has not given.
  await expect(page.getByText("The deadline has been reached")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText(/expired/i)).toHaveCount(0);
  await expectNoInstructions(page);
});

test("a permissionless deposit request shows the document and offers its attachment", async ({
  page,
}) => {
  const errors = watchConsole(page);
  await page.goto("/pay/dr_document");

  const invoice = page.getByRole("region", { name: "Deposit request", exact: true });
  await expect(invoice.getByRole("heading", { name: "Consulting — August" })).toBeVisible();
  await expect(invoice).toContainText("Acme Corp");
  await expect(invoice).toContainText("Globex Corporation");
  await expect(invoice).toContainText("ap@globex.example");
  await expect(invoice).toContainText("PO 7781");
  await expect(invoice).toContainText("INV-1042");
  await expect(invoice).toContainText("25.00 USDC");
  await expect(invoice).toContainText("Net 30. Thank you for your business.");
  await expect(invoice.getByRole("button", { name: /INV-1042\.pdf/ })).toBeVisible();
  // The mechanics are still all there.
  await expect(page.getByText(ADDRESS)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByRole("button", { name: /pay with wallet/i })).toBeVisible();

  expect(errors).toEqual([]);
});

test("the attachment's signed URL is fetched on demand and never server-rendered", async ({
  page,
  request,
}) => {
  const html = await (await request.get("/pay/dr_document")).text();
  expect(html).toContain("INV-1042.pdf");
  expect(html).not.toContain("__download");

  await page.addInitScript(() => {
    (window as unknown as { __opened: string[] }).__opened = [];
    window.open = () => {
      const opened = (window as unknown as { __opened: string[] }).__opened;
      return {
        opener: null,
        location: { replace: (url: string) => opened.push(String(url)) },
        close: () => undefined,
      } as unknown as Window;
    };
  });
  await page.goto("/pay/dr_document");

  const descriptor = page.waitForRequest(/\/v1\/payer\/deposit-requests\/dr_document\/attachment$/);
  await page.getByRole("button", { name: /INV-1042\.pdf/ }).click();
  await descriptor;
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened: string[] }).__opened))
    .toEqual([expect.stringContaining("/__download/")]);
});

const WITHHELD = ["25.00", "USDC", "Globex", "INV-1042", "Net 30", "Payer", ADDRESS, "0x754704Bc"];

async function expectLocked(page: Page, html: string) {
  await expect(page.getByText("Verification required").first()).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Acme Corp");
  await expect(page.getByText("Consulting — August")).toBeVisible();
  await expect(page.getByText("a****@e***.com").first()).toBeVisible();
  await expect(page.getByRole("button", { name: /send a code to/i })).toBeVisible();
  await expectNoInstructions(page);

  // Absence from the tree, not hiding: nothing withheld is in the DOM, the
  // server HTML, or offered as a control. Before a code is requested there is
  // no field at all, and never one for an email address.
  await expect(page.getByRole("region", { name: "Deposit request", exact: true })).toHaveCount(0);
  await expect(page.getByText(/Expires in/)).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
  expect(await page.locator("form, input").count()).toBe(0);
  const text = await page.locator("body").innerText();
  for (const withheld of WITHHELD) {
    expect(text, withheld).not.toContain(withheld);
    // The site's own meta description mentions USDC; every other withheld
    // string would only be in the HTML if the deposit request had put it there.
    if (withheld !== "USDC") expect(html, withheld).not.toContain(withheld);
  }
  expect(html).not.toContain("alice");
}

/** Requests the code and enters it; the stub accepts exactly `123456`. */
async function verifyEmail(page: Page, code = "123456") {
  await page.getByRole("button", { name: /send a code to/i }).click();
  const input = page.getByRole("textbox", { name: /one-time code/i });
  await expect(input).toBeVisible();
  await expect(page.getByText("Check your email")).toBeVisible();
  await input.fill(code);
  await page.getByRole("button", { name: /^verify$/i }).click();
}

test("an email-gated deposit request reveals only the issuer, heading, and masked mailbox", async ({
  page,
  request,
}) => {
  const errors = watchConsole(page);
  const html = await (await request.get("/pay/dr_gated-email")).text();
  await page.goto("/pay/dr_gated-email");

  await expectLocked(page, html);
  await expect(
    page.getByText(/once you verify ownership of the expected credentials/),
  ).toBeVisible();
  await expect(page.getByText(/identity/i)).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("an email-gated deposit request unlocks for the tab that verifies, and only there", async ({
  page,
  request,
  browser,
}) => {
  const errors = watchConsole(page);
  const sessionInUrls: string[] = [];
  page.on("request", (sent) => {
    if (/pps_/.test(sent.url())) sessionInUrls.push(sent.url());
  });
  await page.goto("/pay/dr_gated-email");
  await expectLocked(page, await (await request.get("/pay/dr_gated-email")).text());

  // A wrong code keeps the page locked and says so.
  await verifyEmail(page, "000000");
  await expect(page.getByText(/that code was not accepted/i)).toBeVisible();
  await expectNoInstructions(page);

  await page.getByRole("textbox", { name: /one-time code/i }).fill("123456");
  await page.getByRole("button", { name: /^verify$/i }).click();

  // Everything the gate withheld is now on the page, from this tab's session.
  await expect(page.getByRole("region", { name: "Deposit request", exact: true })).toBeVisible();
  await expect(page.getByText(ADDRESS)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByText("Globex Corporation")).toBeVisible();
  await expect(page.getByRole("button", { name: /INV-1042\.pdf/ })).toBeVisible();
  await expect(page.getByText("Verification required")).toHaveCount(0);

  // The session survives a reload in this tab, but never reaches the server
  // render or a URL.
  await page.reload();
  await expect(page.getByText(ADDRESS)).toBeVisible();
  const html = await (await request.get("/pay/dr_gated-email")).text();
  expect(html).not.toContain(ADDRESS);
  expect(html).not.toContain("pps_");
  expect(sessionInUrls).toEqual([]);
  expect(page.url()).not.toContain("pps_");

  // Another browser holding the same link is still locked.
  const stranger = await browser.newContext();
  const other = await stranger.newPage();
  await other.goto("/pay/dr_gated-email");
  await expectLocked(other, html);
  await stranger.close();

  // The only errors here are the 401 the wrong code earned (the response and
  // the browser's own console line for it); anything else is a fault.
  expect(errors.filter((error) => !/401/.test(error))).toEqual([]);
});

test("the wallet button refuses a deposit request bound on a chain this checkout lacks", async ({
  page,
}) => {
  await page.goto("/pay/dr_other-chain");

  await expect(page.getByText(/This checkout cannot pay on Ethereum/)).toBeVisible();
  await expect(page.getByRole("button", { name: /pay with wallet/i })).toHaveCount(0);
  // The address and QR remain, so the deposit request is still payable by hand.
  await expect(page.getByText(ADDRESS)).toBeVisible();
});

test("an unknown link is a clean dead end", async ({ page }) => {
  const response = await page.goto("/pay/dr_does-not-exist");

  expect(response?.status()).toBe(404);
  await expect(page.getByText("This deposit link is not valid")).toBeVisible();
  await expect(page.getByRole("link", { name: /What is Payday/ })).toBeVisible();
});

test("deposit pages are not indexable and cannot be framed", async ({ page }) => {
  const response = await page.goto("/pay/dr_awaiting");
  const headers = response?.headers() ?? {};

  expect(headers["x-frame-options"]).toBe("DENY");
  expect(headers["referrer-policy"]).toBe("no-referrer");
  const csp = headers["content-security-policy"] ?? "";
  expect(csp).toContain("frame-ancestors 'none'");
  // The checkout renders per request, so it earns a nonce and needs no
  // unsafe-inline for scripts.
  expect(csp).toMatch(/script-src [^;]*'nonce-/);
  expect(csp).not.toMatch(/script-src [^;]*'unsafe-inline'/);
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute("content", /noindex/);
});

test("the static landing page is not served a nonce it cannot satisfy", async ({ page }) => {
  // A nonce minted per request cannot match HTML baked at build time; sending
  // one here would block every script on the page.
  const response = await page.goto("/");
  const csp = response?.headers()["content-security-policy"] ?? "";

  expect(csp).not.toContain("nonce-");
  expect(csp).toContain("frame-ancestors 'none'");
});

test("the landing page renders and is indexable", async ({ page }) => {
  const errors = watchConsole(page);
  const response = await page.goto("/");

  await expect(
    page.getByRole("heading", { name: "Accept stablecoins on your terms." }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Get Started" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Read Docs" })).toHaveAttribute("href", "/docs");
  await expect(page.getByRole("img", { name: "Payday" }).first()).toBeVisible();
  expect(await page.locator('meta[name="robots"]').count()).toBe(0);
  expect(response?.status()).toBe(200);
  expect(errors).toEqual([]);
});

test("the landing page links to public documentation without obsolete environment copy", async ({
  page,
}) => {
  await page.goto("/");
  const body = (await page.locator("body").innerText()).toLowerCase();

  expect(body).not.toContain("sandbox");
  expect(body).not.toContain("quickstart");
  // Public documentation lives in the app itself at /docs, not the repository
  // (see "Publish customer documentation at /docs").
  await expect(page.getByRole("link", { name: "Read Docs" })).toHaveAttribute("href", "/docs");
});

/* ------------------------------------------------------------------------ */
/* Merchant sessions: the merchant's app opens the checkout                 */
/* ------------------------------------------------------------------------ */

const MERCHANT_LINK = "/pay/dr_gated-merchant";
const VALID_SECRET = `cs_${"valid".padEnd(43, "0")}`;
const USED_SECRET = `cs_${"used".padEnd(43, "0")}`;
const OTHER_SECRET = `cs_${"other".padEnd(43, "0")}`;

/** A merchant-session page that nothing has opened: the app is the only way in. */
async function expectAppRequired(page: Page, html: string) {
  await expect(page.getByText("Open from the app").first()).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Acme Corp");
  await expect(page.getByText("Deposit 25 USDC")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: /open this deposit request from acme corp/i }),
  ).toBeVisible();
  await expectNoInstructions(page);
  // Nothing to type, nothing to click, no mailbox: this mode has no step here.
  // (Scoped to the gate: the dev server adds its own overlay button.)
  expect(await page.locator("form, input").count()).toBe(0);
  const gate = page.getByRole("region", { name: /open this deposit request from acme corp/i });
  await expect(gate).toBeVisible();
  await expect(gate.getByRole("button")).toHaveCount(0);
  await expect(gate.getByRole("link")).toHaveCount(0);
  const text = await page.locator("body").innerText();
  expect(text).not.toMatch(/email|one-time code|a\*\*\*\*@/i);
  for (const withheld of WITHHELD) {
    // The heading itself names the amount and asset, as the merchant wrote it;
    // the withheld amount is the formatted "25.00", which must not appear.
    if (withheld === "USDC") continue;
    expect(text, withheld).not.toContain(withheld);
    expect(html, withheld).not.toContain(withheld);
  }
}

test("a merchant-session deposit request opened from its app is unlocked, and the secret never lingers", async ({
  page,
  request,
  browser,
}) => {
  const errors = watchConsole(page);
  const secretInUrls: string[] = [];
  page.on("request", (sent) => {
    if (/cs_valid/.test(sent.url())) secretInUrls.push(sent.url());
  });

  await page.goto(`${MERCHANT_LINK}#${"cs"}=${VALID_SECRET}`);

  // Everything the gate withheld is on the page, from the session the
  // exchange minted, with no step taken by the payer.
  await expect(page.getByRole("region", { name: "Deposit request", exact: true })).toBeVisible();
  await expect(page.getByText(ADDRESS)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByText("Globex Corporation")).toBeVisible();
  await expect(page.getByText("Open from the app")).toHaveCount(0);

  // The secret left the address bar before the exchange answered, never
  // travelled in a request URL, and is not in the server render.
  expect(page.url()).not.toContain("cs_");
  expect(page.url()).not.toContain("#");
  expect(secretInUrls).toEqual([]);
  const html = await (await request.get(MERCHANT_LINK)).text();
  expect(html).not.toContain(ADDRESS);
  expect(html).not.toContain("cs_");

  // The session survives a reload in this tab without the secret.
  await page.reload();
  await expect(page.getByText(ADDRESS)).toBeVisible();

  // Someone else holding the bare link is told to go through the app.
  const stranger = await browser.newContext();
  const other = await stranger.newPage();
  await other.goto(MERCHANT_LINK);
  await expectAppRequired(other, html);
  await stranger.close();

  expect(errors).toEqual([]);
});

test("a merchant-session link opened a second time says so and shows nothing", async ({
  page,
  request,
}) => {
  const errors = watchConsole(page);
  await page.goto(`${MERCHANT_LINK}#cs=${USED_SECRET}`);

  await expect(page.getByText(/this link was already opened/i)).toBeVisible();
  await expect(page.getByText(/go back to acme corp/i)).toBeVisible();
  await expectNoInstructions(page);
  expect(page.url()).not.toContain("cs_");
  expect(await page.locator("form, input").count()).toBe(0);
  const html = await (await request.get(MERCHANT_LINK)).text();
  expect(html).not.toContain(ADDRESS);

  // The only fault is the 409 the spent secret earned.
  expect(errors.filter((error) => !/409/.test(error))).toEqual([]);
});

test("a merchant-session secret for another deposit request is refused", async ({ page }) => {
  const errors = watchConsole(page);
  await page.goto(`${MERCHANT_LINK}#cs=${OTHER_SECRET}`);

  await expect(page.getByText(/expired or is not valid/i)).toBeVisible();
  await expectNoInstructions(page);
  expect(page.url()).not.toContain("cs_");
  expect(errors.filter((error) => !/401/.test(error))).toEqual([]);
});

test("a merchant-session deposit request opened without its app asks for the app", async ({
  page,
  request,
}) => {
  const errors = watchConsole(page);
  const html = await (await request.get(MERCHANT_LINK)).text();
  await page.goto(MERCHANT_LINK);

  await expectAppRequired(page, html);
  expect(errors).toEqual([]);
});
