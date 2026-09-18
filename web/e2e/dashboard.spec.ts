import { expect, test, type Page } from "@playwright/test";

/**
 * The dashboard against e2e/stub-api.mjs, which plays the merchant API and
 * the presigned upload target, with `test/privy-stub.tsx` standing in for
 * Privy. Each test signs in on its own: the session lives in the tab, and the
 * stub accepts one code.
 *
 * Signing in is the landing page's dialog — there is no dashboard login page —
 * and it is covered on its own in signup.spec.ts.
 */

const OTP = "123456";

async function signIn(page: Page, email = "merchant@example.com") {
  await page.goto("/");
  await page.getByRole("button", { name: "Get Started" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email").fill(email);
  await dialog.getByRole("button", { name: "Send code" }).click();
  await dialog.getByLabel("One-time code").fill(OTP);
  await dialog.getByRole("button", { name: "Continue" }).click();
  // Everything a merchant does day to day is on this one page.
  await expect(page).toHaveURL(/\/dashboard$/);
}

test("a signed-out visitor is sent to the landing page and the shell carries no data", async ({
  page,
  request,
}) => {
  const response = await request.get("/dashboard/deposits");
  const html = await response.text();
  expect(response.status()).toBe(200);
  expect(html).toContain('name="robots"');
  expect(html).toContain("noindex");
  // Seeded deposit requests exist at the API; none of them is in the HTML.
  expect(html).not.toContain("Consulting — August");
  expect(html).not.toContain("Globex");
  expect(html).not.toContain("stub-dashboard-token");

  // Every dashboard route, list or detail, sends a visitor without a session
  // to the landing page, where the only way in is.
  await page.goto("/dashboard");
  await expect(page).toHaveURL("/");
  await page.goto("/dashboard/deposits/dr_seed-settled");
  await expect(page).toHaveURL("/");
});

test("the deposit list shows verification separately from the deposit and flags unsolicited funds", async ({
  page,
}) => {
  await signIn(page);

  const settled = page.getByRole("row").filter({ hasText: "Consulting — August" });
  await expect(settled).toContainText("Globex Corporation");
  await expect(settled).toContainText("25.00");
  await expect(settled).toContainText("Settled");
  await expect(settled).toContainText("Verified email");
  await expect(settled).toContainText("Verified");
  await expect(settled.getByLabel("Has attachment")).toBeVisible();

  const unsolicited = page.getByRole("row").filter({ hasText: "Retainer — September" });
  await expect(unsolicited).toContainText("Deposited");
  await expect(unsolicited).toContainText("Pending");
  await expect(unsolicited).toContainText("Likely unsolicited");

  // The filters are the API's own parameters, not a client-side sieve, and
  // verification narrows separately from deposit status.
  await page.getByLabel("Status").click();
  await page.getByRole("option", { name: "Settled" }).click();
  await expect(page.getByRole("row").filter({ hasText: "Retainer — September" })).toHaveCount(0);
  await expect(page.getByRole("row").filter({ hasText: "Consulting — August" })).toHaveCount(1);

  await page.getByLabel("Status").click();
  await page.getByRole("option", { name: "Any status" }).click();
  await page.getByLabel("Verification").click();
  await page.getByRole("option", { name: "Pending" }).click();
  await expect(page.getByRole("row").filter({ hasText: "Consulting — August" })).toHaveCount(0);
  await expect(page.getByRole("row").filter({ hasText: "Retainer — September" })).toHaveCount(1);
});

test("a settled invoice offers its PDF, its Proof of Payment, and its recovered funds", async ({
  page,
}) => {
  await signIn(page);
  await page.getByRole("button", { name: /Consulting — August/ }).click();

  await expect(page.getByText("Settled").filter({ visible: true }).first()).toBeVisible();
  await expect(page.getByText("Verified", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("alice@globex.example")).toBeVisible();

  // Both the overpayment remainder and the late transfer went back to the payer.
  const recovered = page.getByRole("region", { name: "Returned to the payer" });
  await expect(recovered).toContainText("Overpayment remainder");
  await expect(recovered).toContainText("5.00 USDC");
  await expect(recovered).toContainText("Late transfer");
  await expect(recovered).toContainText("collected");

  const proofDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Proof of Payment" }).click();
  const proof = await proofDownload;
  expect(proof.suggestedFilename()).toBe("INV-1042-proof.json");
  const body = JSON.parse((await streamToString(proof)) ?? "");
  expect(body.version).toBe("payday.proof.v4");
  expect(body.payer_wallet.typed_data.primaryType).toBe("PayerAttestation");
  expect(body.recovery_address).toBe(body.payer_wallet.address);
  expect(body.payment_id).toBe("dr_seed-settled");
  expect(body.canonical_issuance_snapshot.attachment.sha256).toMatch(/^0x[0-9a-f]{64}$/);
  expect(body.verification.signer).toBe("0x976EA74026E726554dB657fA54763abd0C3a0aa9");

  const pdfDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Deposit request PDF" }).click();
  const pdf = await pdfDownload;
  expect(pdf.suggestedFilename()).toBe("INV-1042.pdf");
  expect((await streamToString(pdf))?.startsWith("%PDF-")).toBe(true);
});

test("a likely unsolicited deposit shows its flag and every verification attempt", async ({
  page,
}) => {
  await signIn(page);
  await page.getByRole("button", { name: /Retainer — September/ }).click();

  await expect(page.getByText("Likely unsolicited").first()).toBeVisible();
  await expect(page.getByText("bob@initech.example")).toBeVisible();
  const activity = page.getByLabel("Verification activity");
  await expect(activity).toBeVisible();
  await expect(activity.getByText("Email verification")).toHaveCount(2);
  await expect(activity).toContainText("Abandoned");
  await expect(activity).toContainText("Code sent");
  const text = await page.locator("body").innerText();
  expect(text).not.toMatch(/SENTINEL|1900-01-01|identity check|identity document|risk/i);
});

test("the account section shows the signed-in mailbox and the Gum wallet", async ({ page }) => {
  await signIn(page, "account-view@example.com");

  const section = page.getByRole("region", { name: "Account" });
  await expect(section).toContainText("account-view@example.com");
  // The wallet is the account's own, shown in full and ready to copy.
  const wallet = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("payday.privy-stub.session") ?? "{}").wallet,
  );
  expect(wallet).toMatch(/^0x[0-9a-f]{40}$/);
  await expect(section).toContainText(wallet);
  await expect(section.getByRole("button", { name: /Copy wallet address/ })).toBeVisible();
  // One balance per network the deployment offers, each read on its own from
  // the stub RPC, which answers with each chain's stub balance.
  await expect(section.getByText(/5\.00\s*USDC/)).toBeVisible({ timeout: 20_000 });
  await expect(section.getByText(/3\.00\s*USDT0/)).toBeVisible({ timeout: 20_000 });
  await expect(section.getByText(/1\.25\s*USDC/)).toBeVisible({ timeout: 20_000 });
});

test("a merchant can generate, roll, and revoke their API key from the session", async ({
  page,
}) => {
  await signIn(page, "api-key-flow@example.com");

  const section = page.getByRole("region", { name: "API key" });
  await expect(section).toContainText("No key yet");

  // A first key: one confirmation, no second sign-in.
  await section.getByRole("button", { name: "Generate key" }).click();
  await expect(section.getByText(/shown here once and never again/)).toBeVisible();
  await section.getByRole("button", { name: "Confirm & generate" }).click();

  await expect(section.getByText("Key generated")).toBeVisible();
  const firstKey = await section.locator("code").innerText();
  expect(firstKey).toMatch(/^payday_test_stub/);
  await section.getByRole("button", { name: "Done" }).click();
  await expect(section).toContainText(firstKey.slice(-6));
  await expect(section.getByRole("button", { name: "Roll key" })).toBeVisible();

  // Rolling warns about the grace period up front, then reveals a new key.
  await section.getByRole("button", { name: "Roll key" }).click();
  await expect(section.getByText(/current one keeps working for 24 hours/)).toBeVisible();
  await section.getByRole("button", { name: "Confirm & roll key" }).click();

  await expect(section.getByText("Key rolled")).toBeVisible();
  const secondKey = await section.locator("code").innerText();
  expect(secondKey).not.toBe(firstKey);
  await expect(section.getByText(/previous key keeps working for the next 24 hours/)).toBeVisible();
  await section.getByRole("button", { name: "Done" }).click();
  await expect(section.getByText(/previous key still works until/)).toBeVisible();

  // Revoking warns that it is immediate and irreversible before confirming.
  await section.getByRole("button", { name: "Revoke" }).click();
  await expect(section.getByText(/can't be undone/)).toBeVisible();
  await section.getByRole("button", { name: "Confirm & revoke" }).click();

  await expect(section.getByText("Key revoked.")).toBeVisible();
  await section.getByRole("button", { name: "Done" }).click();
  await expect(section).toContainText("Revoked");
  await expect(section.getByRole("button", { name: "Generate key" })).toBeVisible();
});

test("signing out ends the session", async ({ page }) => {
  await signIn(page);
  await page.getByRole("button", { name: "Sign out" }).first().click();
  await expect(page).toHaveURL("/");
  expect(await page.evaluate(() => sessionStorage.getItem("payday.privy-stub.session"))).toBeNull();
  // And the dashboard is closed again.
  await page.goto("/dashboard");
  await expect(page).toHaveURL("/");
});

async function streamToString(download: {
  createReadStream: () => Promise<NodeJS.ReadableStream>;
}) {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(Buffer.from(chunk as Buffer));
  return Buffer.concat(chunks).toString("utf8");
}

test("a merchant withdraws everything to one network from the account section", async ({
  page,
}) => {
  // The stub relayer advances one state per read and the page reads every ten
  // seconds, so a bridge leg takes about a minute to land.
  test.setTimeout(150_000);
  await signIn(page, "withdraw-flow@example.com");
  const section = page.getByRole("region", { name: "Account" });
  await expect(section.getByRole("button", { name: "Export wallet key" })).toBeVisible();

  // Prepare: pick Base as the destination and name an address.
  await section.getByRole("button", { name: "Withdraw", exact: true }).click();
  await section.getByRole("radio", { name: /Base/ }).click();
  const address = section.getByRole("textbox", { name: "Destination address" });
  await address.fill("0xnope");
  await expect(section.getByRole("alert")).toContainText("Not a valid address");
  await expect(section.getByRole("button", { name: "Continue" })).toBeDisabled();
  await address.fill("0x000000000000000000000000000000000000d00d");
  await section.getByRole("button", { name: "Continue" }).click();

  // Review: one leg per network the stub wallet holds USDC on. The Monad
  // leg bridges through the forwarder; the Base leg is a plain transfer.
  await expect(section.getByText("2 legs to Base")).toBeVisible();
  const legs = section.getByRole("list", { name: "Legs" }).getByRole("listitem");
  await expect(legs).toHaveCount(2);
  await expect(legs.nth(0)).toContainText("5.00 USDC");
  await expect(legs.nth(0)).toContainText("Monad → Base via CCTP");
  await expect(legs.nth(0)).toContainText("Pays Gum's forwarder");
  await expect(legs.nth(1)).toContainText("1.25 USDC");
  await expect(legs.nth(1)).toContainText("on Base");

  // Sign: the page checks each document, asks the wallet once per leg, and
  // hands the signatures back. The stub relayer then advances a state per
  // read, and the page polls every ten seconds until every leg has landed.
  await section.getByRole("button", { name: "Sign and withdraw" }).click();
  await expect(section.getByText("Withdrawing…")).toBeVisible();
  await expect(section.getByText(/Bridged legs wait for Circle/)).toBeVisible();
  await expect(section.getByText("Withdrawn to Base.")).toBeVisible({ timeout: 90_000 });
  await expect(legs.nth(0).getByRole("link", { name: "Burn" })).toBeVisible();
  await expect(legs.nth(0).getByRole("link", { name: "Mint" })).toBeVisible();
  await expect(legs.nth(1).getByRole("link", { name: "Transfer" })).toBeVisible();
  await section.getByRole("button", { name: "Done" }).click();

  // The finished withdrawal is listed, and a new one can start.
  const history = section.getByRole("list", { name: "Recent withdrawals" });
  await expect(history.getByRole("listitem")).toHaveCount(1);
  await expect(history).toContainText("Completed");
  await expect(history).toContainText("5.00 USDC from Monad");
  await expect(section.getByRole("button", { name: "Withdraw", exact: true })).toBeEnabled();
});

test("a withdrawal can be cancelled before it is signed", async ({ page }) => {
  await signIn(page, "withdraw-cancel@example.com");
  const section = page.getByRole("region", { name: "Account" });
  await section.getByRole("button", { name: "Withdraw", exact: true }).click();
  await section
    .getByRole("textbox", { name: "Destination address" })
    .fill("0x000000000000000000000000000000000000d00d");
  await section.getByRole("button", { name: "Continue" }).click();
  await expect(section.getByText(/legs? to Monad/)).toBeVisible();
  await section.getByRole("button", { name: "Cancel withdrawal" }).click();
  await expect(section.getByRole("button", { name: "Withdraw", exact: true })).toBeVisible();
  const history = section.getByRole("list", { name: "Recent withdrawals" });
  await expect(history).toContainText("Cancelled");
});

test("a USDT withdrawal moves that network's balance alone", async ({ page }) => {
  await signIn(page, "usdt@example.com");

  // The account section shows each network's balance in every stablecoin it
  // serves: Monad holds USDT0 next to its USDC.
  const section = page.getByRole("region", { name: "Account" });
  await expect(section.getByText(/3\.00\s*USDT0/)).toBeVisible({ timeout: 20_000 });

  // A USDT withdrawal offers only the network serving it, and moves that
  // network's balance in one transfer leg: nothing bridges.
  await section.getByRole("button", { name: "Withdraw", exact: true }).click();
  await section.getByRole("radiogroup", { name: "Currency" }).getByRole("radio", { name: /USDT/ }).click();
  const destinations = section.getByRole("radiogroup", { name: "Withdraw to" });
  await expect(destinations.getByRole("radio", { name: /Monad/ })).toBeVisible();
  await expect(destinations.getByRole("radio", { name: /Base/ })).toHaveCount(0);
  await section
    .getByRole("textbox", { name: "Destination address" })
    .fill("0x000000000000000000000000000000000000d00d");
  await section.getByRole("button", { name: "Continue" }).click();
  await expect(section.getByText("One leg to Monad")).toBeVisible();
  const legs = section.getByRole("list", { name: "Legs" }).getByRole("listitem");
  await expect(legs).toHaveCount(1);
  await expect(legs.nth(0)).toContainText("3.00 USDT0");
  await expect(legs.nth(0)).toContainText("on Monad");
  await section.getByRole("button", { name: "Cancel withdrawal" }).click();
  await expect(section.getByRole("button", { name: "Withdraw", exact: true })).toBeVisible();
});
