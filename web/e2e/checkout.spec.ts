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

async function expectNoInstructions(page: Page) {
  await expect(page.getByText(ADDRESS)).toHaveCount(0);
  await expect(page.getByRole("img", { name: /QR code/i })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /pay with wallet/i })).toHaveCount(0);
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
  await expect(page.getByRole("img", { name: /QR code/i })).toHaveAttribute("src", /v=15000000$/);
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
    page.getByText(/above the invoice amount went to the Payday recovery wallet/),
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
  await expect(
    page.getByText(/recovery wallet, which is not automatically the payer/),
  ).toBeVisible();
  await expectNoInstructions(page);
});

test("a returned payment does the same", async ({ page }) => {
  await page.goto("/pay/pay_returned");

  await expect(page.getByText("This payment was not completed in time")).toBeVisible();
  await expect(page.getByText(/recovery wallet/)).toBeVisible();
  await expectNoInstructions(page);
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

  const invoice = page.getByRole("region", { name: "Invoice" });
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
    window.open = (url) => {
      (window as unknown as { __opened: string[] }).__opened.push(String(url));
      return window;
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
  await expect(page.getByText("a****@e***.com")).toBeVisible();
  await expect(page.getByText(/verification is being enabled/i)).toBeVisible();
  await expectNoInstructions(page);

  // Absence from the tree, not hiding: nothing withheld is in the DOM, the
  // server HTML, or offered as a control.
  await expect(page.getByRole("region", { name: "Invoice" })).toHaveCount(0);
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

test("an email-gated invoice reveals only the issuer, heading, and masked mailbox", async ({
  page,
  request,
}) => {
  const errors = watchConsole(page);
  const html = await (await request.get("/pay/pay_gated-email")).text();
  await page.goto("/pay/pay_gated-email");

  await expectLocked(page, html);
  await expect(page.getByText("Email ownership")).toBeVisible();
  await expect(page.getByText("Identity document")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("identity-gated invoices say which checks are needed, matched or not", async ({
  page,
  request,
}) => {
  await page.goto("/pay/pay_gated-identity");
  await expectLocked(page, await (await request.get("/pay/pay_gated-identity")).text());
  await expect(page.getByText("Identity document")).toBeVisible();
  await expect(page.getByText("Name matches the invoice")).toBeVisible();
  await expect(page.getByText(/matching the person it names/)).toBeVisible();

  await page.goto("/pay/pay_gated-unattributed");
  await expectLocked(page, await (await request.get("/pay/pay_gated-unattributed")).text());
  await expect(page.getByText("Identity document")).toBeVisible();
  await expect(page.getByText("Name matches the invoice")).toHaveCount(0);
  await expect(page.getByText(/matching the person it names/)).toHaveCount(0);
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
    page.getByRole("heading", { name: "Minimal API to accept and catalog stablecoin payments." }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "How it works" })).toBeVisible();
  await expect(page.getByRole("img", { name: "Payday" }).first()).toBeVisible();
  expect(await page.locator('meta[name="robots"]').count()).toBe(0);
  expect(response?.status()).toBe(200);
  expect(errors).toEqual([]);
});

test("the landing page offers no sandbox or quickstart", async ({ page }) => {
  await page.goto("/");
  const body = (await page.locator("body").innerText()).toLowerCase();

  expect(body).not.toContain("sandbox");
  expect(body).not.toContain("quickstart");
  await expect(page.getByRole("link", { name: "Docs" })).toHaveCount(0);
});

test("the hero terminal cycles through the CLI flows", async ({ page }) => {
  await page.goto("/");

  // Every flow is always listed; only the running one is highlighted.
  await expect(page.locator("[data-scene]")).toHaveText(["create", "list", "get --watch"]);

  const active = page.locator('[data-scene][data-active="true"]');
  // Server-rendered on the first scene, so it is never a blank box.
  await expect(active).toHaveText("create");
  await expect(active).toHaveText("list", { timeout: 15_000 });
  await expect(active).toHaveText("get --watch", { timeout: 20_000 });
  // The watch scene redraws in place, ending on a settled payment.
  await expect(page.getByText(/Settled at/)).toBeVisible({ timeout: 20_000 });
});

test("the hero terminal never changes size as scenes change", async ({ page }) => {
  await page.goto("/");
  const panel = page.locator("[data-scene]").first().locator("xpath=ancestor::div[2]");

  const heights = new Set<number>();
  for (let tick = 0; tick < 30; tick++) {
    const box = await panel.boundingBox();
    if (box) heights.add(Math.round(box.height));
    await page.waitForTimeout(500);
  }

  // A window that resizes mid-scene shifts everything below it on the page.
  expect([...heights]).toHaveLength(1);
});
