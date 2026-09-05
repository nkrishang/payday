import { expect, test, type Page } from "@playwright/test";

/**
 * Every scenario is selected by the payment id; see e2e/stub-api.mjs.
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
      const provider = {
        isFakeWallet: true,
        request: async ({ method, params }: { method: string; params?: unknown[] }) => {
          switch (method) {
            case "eth_requestAccounts":
            case "eth_accounts":
              return [wallet];
            case "eth_chainId":
              return "0x8f";
            case "wallet_switchEthereumChain":
              return null;
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
      Object.assign(window, { ethereum: provider, __signed: signed });
    },
    { wallet: PAYER_WALLET },
  );
}

test("an open payment shows everything needed to pay it", async ({ page }) => {
  const errors = watchConsole(page);
  await page.goto("/pay/pay_awaiting");

  await expect(page.getByText("Awaiting payment")).toBeVisible();
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
  await page.goto("/pay/pay_awaiting");

  const countdown = page.getByText(/Expires in/);
  const first = await countdown.textContent();
  await expect(async () => {
    expect(await countdown.textContent()).not.toBe(first);
  }).toPass({ timeout: 5_000 });

  // A ticking countdown means React took over the server-rendered markup.
  expect(errors).toEqual([]);
});

test("a partial payment asks for the remainder and re-codes the QR", async ({ page }) => {
  await page.goto("/pay/pay_partial");

  await expect(page.getByText("Partially paid")).toBeVisible();
  await expect(page.getByText("Remaining", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("15.00");
  await expect(page.getByText("10.00 of 25.00 USDC received")).toBeVisible();
  // The card is labelled with what is being asked for, for assistive tech.
  await expect(page.getByRole("region", { name: "Send the remaining 15.00 USDC" })).toBeVisible();
  await expect(page.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "40");
  // The QR is fetched again for the new remainder and shown from an object
  // URL: nothing about the payment or the session is in the image URL.
  await expect(page.getByRole("img", { name: /QR code to pay 15/i })).toHaveAttribute(
    "src",
    /^blob:/,
  );
});

test("polling moves the page without a reload", async ({ page, request }) => {
  await request.get("http://127.0.0.1:4010/__reset");
  await page.goto("/pay/pay_transition");

  await expect(page.getByText("Awaiting payment")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("25.00");
  // The next read reports a partial payment; nothing here reloads.
  await expect(page.getByText("Partially paid")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("heading", { level: 1 })).toContainText("15.00");
});

test("a settled payment is a receipt, not an invitation to pay again", async ({ page }) => {
  await page.goto("/pay/pay_settled");

  await expect(page.getByText("Payment complete")).toBeVisible();
  await expect(page.getByText("Exactly the invoice amount reached the merchant.")).toBeVisible();
  // An exact payment has nothing in recovery, so the receipt must not mention it.
  await expect(page.getByText(/recovery wallet/)).toHaveCount(0);
  await expect(page.getByRole("link", { name: /0x00210b33/ })).toBeVisible();
  await expectNoInstructions(page);
});

test("an overpaid settled payment says where the remainder went", async ({ page }) => {
  await page.goto("/pay/pay_settled-overpaid");

  await expect(page.getByText("Payment complete")).toBeVisible();
  await expect(page.getByText(/Exactly the invoice amount reached the merchant/)).toBeVisible();
  await expect(
    page.getByText(/above the invoice amount went back to the wallet you signed with/),
  ).toBeVisible();
  // The receipt shows both what was asked for and what actually arrived.
  await expect(page.getByText("25.00 USDC")).toBeVisible();
  await expect(page.getByText("30.00 USDC")).toBeVisible();
  await expectNoInstructions(page);
});

test("a received payment says settlement is still in progress", async ({ page }) => {
  await page.goto("/pay/pay_paid");

  await expect(page.getByText("Payment received")).toBeVisible();
  await expect(page.getByText(/settling the invoice amount to the merchant/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an expired payment that received nothing asks for a new link", async ({ page }) => {
  await page.goto("/pay/pay_expired");

  await expect(page.getByText("This payment link has expired")).toBeVisible();
  await expect(page.getByText(/Ask the merchant for a new payment link/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an expired payment holding funds says where they went", async ({ page }) => {
  await page.goto("/pay/pay_expired-funded");

  await expect(page.getByText("The deadline passed before this payment completed")).toBeVisible();
  await expect(page.getByText(/goes back to the wallet you signed with/)).toBeVisible();
  await expectNoInstructions(page);
});

test("a returned payment does the same", async ({ page }) => {
  await page.goto("/pay/pay_returned");

  await expect(page.getByText("This payment was not completed in time")).toBeVisible();
  await expect(page.getByText(/back to the wallet you signed with/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an unbound request takes the payer's signature before it shows any address", async ({
  page,
  request,
}) => {
  await installFakeWallet(page);
  const sessionInUrls: string[] = [];
  page.on("request", (sent) => {
    if (/pps_/.test(sent.url())) sessionInUrls.push(sent.url());
  });
  await page.goto("/pay/pay_unbound");

  // The content is open, the address is not: nothing invites a transfer yet.
  await expect(page.getByText("Wallet required")).toBeVisible();
  await expect(page.getByText("Amount due")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("25.00");
  await expect(page.getByText(/Only transfers from that wallet count/)).toBeVisible();
  await expectNoInstructions(page);
  const html = await (await request.get("/pay/pay_unbound")).text();
  expect(html).not.toContain(ADDRESS);

  // Connect the fake wallet, then sign the challenge it is handed.
  await page.getByRole("button", { name: /connect the wallet you will pay from/i }).click();
  await page.getByRole("dialog").getByRole("button").filter({ hasText: /injected/i }).click();
  await expect(page.getByRole("button", { name: /sign to get your deposit address/i })).toBeVisible();
  await page.getByRole("button", { name: /sign to get your deposit address/i }).click();

  // The signature created the address; the page now offers it, tied to the
  // wallet that signed.
  await expect(page.getByText(ADDRESS)).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText(/Send from/)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByText("Wallet required")).toHaveCount(0);
  // What the wallet was asked to sign is the API's document, verbatim.
  const signed = await page.evaluate(() => (window as unknown as { __signed: unknown[] }).__signed);
  expect(signed).toHaveLength(1);
  const [signer, json] = signed[0] as [string, string];
  expect(signer.toLowerCase()).toBe(PAYER_WALLET.toLowerCase());
  const typed = JSON.parse(json);
  expect(typed.primaryType).toBe("PayerAttestation");
  expect(typed.domain.chainId).toBe(143);
  expect(typed.message.wallet.toLowerCase()).toBe(PAYER_WALLET.toLowerCase());
  expect(typed.message.statement).toContain("Only transfers from this wallet");
  // The session never reached a URL.
  expect(sessionInUrls).toEqual([]);
});

test("a paused payment shows the gateway's own words", async ({ page }) => {
  await page.goto("/pay/pay_attention");

  await expect(page.getByText("Settlement is paused")).toBeVisible();
  await expect(page.getByText(/your funds remain safe/)).toBeVisible();
  await expect(page.getByText(/do not send a second payment/)).toBeVisible();
  await expectNoInstructions(page);
});

test("an address stops being offered the moment the gateway says it is not payable", async ({
  page,
}) => {
  await page.goto("/pay/pay_closing");

  await expect(page.getByText("The deadline has been reached")).toBeVisible();
  await expect(page.getByText(/Do not send funds now/)).toBeVisible();
  await expectNoInstructions(page);
});

test("the countdown reaching zero does not itself declare the payment expired", async ({
  page,
}) => {
  await page.goto("/pay/pay_ending");

  await expect(page.getByRole("button", { name: /pay with wallet/i })).toBeVisible();

  // Chain time decides expiry. When the deadline passes, the page stops
  // offering the address but must not claim an outcome the server has not given.
  await expect(page.getByText("The deadline has been reached")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText(/expired/i)).toHaveCount(0);
  await expectNoInstructions(page);
});

test("a permissionless invoice shows the document and offers its attachment", async ({ page }) => {
  const errors = watchConsole(page);
  await page.goto("/pay/pay_invoice");

  const invoice = page.getByRole("region", { name: "Invoice", exact: true });
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
  const html = await (await request.get("/pay/pay_invoice")).text();
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
  await page.goto("/pay/pay_invoice");

  const descriptor = page.waitForRequest(/\/v1\/payer\/payments\/pay_invoice\/attachment$/);
  await page.getByRole("button", { name: /INV-1042\.pdf/ }).click();
  await descriptor;
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened: string[] }).__opened))
    .toEqual([expect.stringContaining("/__download/")]);
});

const WITHHELD = [
  "25.00",
  "USDC",
  "Globex",
  "INV-1042",
  "Net 30",
  "Bill to",
  ADDRESS,
  "0x754704Bc",
];

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
  await expect(page.getByRole("region", { name: "Invoice", exact: true })).toHaveCount(0);
  await expect(page.getByText(/Expires in/)).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
  expect(await page.locator("form, input").count()).toBe(0);
  const text = await page.locator("body").innerText();
  for (const withheld of WITHHELD) {
    expect(text, withheld).not.toContain(withheld);
    // The site's own meta description mentions USDC; every other withheld
    // string would only be in the HTML if the payment had put it there.
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

test("an email-gated invoice reveals only the issuer, heading, and masked mailbox", async ({
  page,
  request,
}) => {
  const errors = watchConsole(page);
  const html = await (await request.get("/pay/pay_gated-email")).text();
  await page.goto("/pay/pay_gated-email");

  await expectLocked(page, html);
  await expect(page.getByText(/once the payer verifies the email address/)).toBeVisible();
  await expect(page.getByText(/identity/i)).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("an email-gated invoice unlocks for the tab that verifies, and only there", async ({
  page,
  request,
  browser,
}) => {
  const errors = watchConsole(page);
  const sessionInUrls: string[] = [];
  page.on("request", (sent) => {
    if (/pps_/.test(sent.url())) sessionInUrls.push(sent.url());
  });
  await page.goto("/pay/pay_gated-email");
  await expectLocked(page, await (await request.get("/pay/pay_gated-email")).text());

  // A wrong code keeps the page locked and says so.
  await verifyEmail(page, "000000");
  await expect(page.getByText(/that code was not accepted/i)).toBeVisible();
  await expectNoInstructions(page);

  await page.getByRole("textbox", { name: /one-time code/i }).fill("123456");
  await page.getByRole("button", { name: /^verify$/i }).click();

  // Everything the gate withheld is now on the page, from this tab's session.
  await expect(page.getByRole("region", { name: "Invoice", exact: true })).toBeVisible();
  await expect(page.getByText(ADDRESS)).toBeVisible();
  await expect(page.getByRole("img", { name: /QR code/i })).toBeVisible();
  await expect(page.getByText("Globex Corporation")).toBeVisible();
  await expect(page.getByRole("button", { name: /INV-1042\.pdf/ })).toBeVisible();
  await expect(page.getByText("Verification required")).toHaveCount(0);

  // The session survives a reload in this tab, but never reaches the server
  // render or a URL.
  await page.reload();
  await expect(page.getByText(ADDRESS)).toBeVisible();
  const html = await (await request.get("/pay/pay_gated-email")).text();
  expect(html).not.toContain(ADDRESS);
  expect(html).not.toContain("pps_");
  expect(sessionInUrls).toEqual([]);
  expect(page.url()).not.toContain("pps_");

  // Another browser holding the same link is still locked.
  const stranger = await browser.newContext();
  const other = await stranger.newPage();
  await other.goto("/pay/pay_gated-email");
  await expectLocked(other, html);
  await stranger.close();

  // The only errors here are the 401 the wrong code earned (the response and
  // the browser's own console line for it); anything else is a fault.
  expect(errors.filter((error) => !/401/.test(error))).toEqual([]);
});


test("the wallet button refuses a payment for another chain", async ({ page }) => {
  await page.goto("/pay/pay_other-chain");

  await expect(
    page.getByText(/configured for Monad, but the payment asks for Ethereum/),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: /pay with wallet/i })).toHaveCount(0);
  // The address and QR remain, so the payment is still payable by hand.
  await expect(page.getByText(ADDRESS)).toBeVisible();
});

test("an unknown link is a clean dead end", async ({ page }) => {
  const response = await page.goto("/pay/pay_does-not-exist");

  expect(response?.status()).toBe(404);
  await expect(page.getByText("This payment link is not valid")).toBeVisible();
  await expect(page.getByRole("link", { name: /What is Payday/ })).toBeVisible();
});

test("payment pages are not indexable and cannot be framed", async ({ page }) => {
  const response = await page.goto("/pay/pay_awaiting");
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
    page.getByRole("heading", { name: "Make every stablecoin accountable." }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Start Building" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Request a demo" })).toBeVisible();
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
  await expect(page.getByRole("link", { name: "Docs" })).toHaveAttribute(
    "href",
    "https://github.com/nkrishang/payday/tree/main/docs",
  );
});

test("the payment carousel advances upward and settles with a bounce", async ({ page }) => {
  await page.goto("/");

  const track = page.locator(".scene-payment-track");
  await expect(track.locator(".payment-row").first()).toBeVisible();

  const positions = await track.evaluate((element) => {
    const animation = element
      .getAnimations()
      .find((candidate) => candidate.effect?.getTiming().duration === 36_000);
    if (!animation) return [];

    animation.pause();
    return [360, 720, 1_080].map((time) => {
      animation.currentTime = time;
      return new DOMMatrix(getComputedStyle(element).transform).m42;
    });
  });

  expect(positions).toHaveLength(3);
  expect(positions[1]).toBeLessThan(positions[0]!);
  expect(positions[2]).toBeGreaterThan(positions[1]!);
});

test("the payment scene stays fixed as its verification panel opens and closes", async ({
  page,
}) => {
  await page.goto("/");
  const stage = page.locator(".payment-stage");

  const snapshots = await stage.evaluate((element) => {
    const animations = element.getAnimations({ subtree: true });
    const panel = element.querySelector<HTMLElement>(".checkout-panel")!;

    animations.forEach((animation) => animation.pause());
    return [0, 9_000, 27_000, 35_999].map((time) => {
      animations.forEach((animation) => (animation.currentTime = time));
      const bounds = element.getBoundingClientRect();
      return {
        width: Math.round(bounds.width),
        height: Math.round(bounds.height),
        panelOpacity: Number(getComputedStyle(panel).opacity),
      };
    });
  });

  expect(new Set(snapshots.map(({ width }) => width)).size).toBe(1);
  expect(new Set(snapshots.map(({ height }) => height)).size).toBe(1);
  expect(snapshots.map(({ panelOpacity }) => panelOpacity)).toEqual([0, 1, 1, 0]);
});
