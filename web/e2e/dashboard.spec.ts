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

const STUB = "http://127.0.0.1:4010";
const OTP = "123456";
const PDF = Buffer.from(
  "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n",
);

async function signIn(page: Page, email = "merchant@example.com") {
  await page.goto("/");
  await page.getByRole("button", { name: "Start Building" }).click();
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

test("a merchant can create a customer, upload a PDF, issue a request, and open it", async ({
  page,
}) => {
  await signIn(page);

  // An identity first: nothing is issued without one, and the composer is the
  // only way in — there is no separate deposit request form. Where it settles needs
  // no setting up: the account's own wallet is the default.
  await page.getByRole("button", { name: "New deposit request" }).click();
  await page.getByLabel("Issued by").fill("Acme Corp");
  await page.getByLabel("Contact address").fill("billing@acme.example");
  await page.getByRole("button", { name: "Send code" }).click();
  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Confirm code" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();

  // Customer, from the dashboard's own customers section.
  await page.getByRole("button", { name: "New customer" }).click();
  const customerName = `Initrode ${Date.now()}`;
  await page.getByLabel("Name").fill(customerName);
  await page.getByLabel("Email").fill("ap@initrode.example");
  await page.getByLabel("Details").fill("Vendor #4471");
  await page.getByRole("button", { name: "Create customer" }).click();
  await expect(page).toHaveURL(/\/dashboard\/customers\/cus_[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { name: customerName })).toBeVisible();

  // Straight into a request for that customer, which opens on them.
  await page.getByRole("link", { name: "New deposit request for this customer" }).click();
  await expect(page.getByRole("heading", { name: "New deposit request." })).toBeVisible();

  // Amount, and a deadline of the merchant's own choosing rather than a preset.
  await page.getByLabel("Amount").fill("120.50");
  await page.getByRole("radio", { name: "Custom" }).click();
  const deadline = new Date(Date.now() + 3 * 24 * 3600_000);
  const pad = (value: number) => String(value).padStart(2, "0");
  const chosen =
    `${deadline.getFullYear()}-${pad(deadline.getMonth() + 1)}-${pad(deadline.getDate())}` +
    `T${pad(deadline.getHours())}:${pad(deadline.getMinutes())}`;
  await page.getByLabel("Date and time").fill(chosen);
  await page.getByRole("button", { name: "Continue" }).click();

  // Billing: the customer came through the link, and the PDF is asked for here.
  await expect(page.getByLabel("Payer")).toHaveValue(customerName);
  await expect(page.getByLabel("Email")).toHaveValue("ap@initrode.example");
  await page.getByLabel("Reason").fill("Design retainer");
  await page.getByLabel("Reference").fill("INV-2001");
  await page.getByLabel("Notes").fill("Net 15.");

  // Attachment: upload, scan, ready.
  const uploads = page.waitForRequest(
    (request) => request.method() === "PUT" && request.url().includes("/__upload/"),
  );
  await page.getByLabel(/Attachment \(PDF/).setInputFiles({
    name: "retainer.pdf",
    mimeType: "application/pdf",
    buffer: PDF,
  });
  const put = await uploads;
  expect(put.headers()["content-type"]).toBe("application/pdf");
  expect(put.headers()["x-amz-tagging"]).toBe("payday-upload=pending");
  expect(put.headers()["authorization"]).toBeUndefined();
  await expect(page.getByRole("status")).toContainText(/Scanning retainer\.pdf/);
  await expect(page.getByText(/Ready · /)).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "Continue" }).click();

  // A gated policy with its assertion; the expected mailbox arrives filled in.
  await page.getByRole("radio", { name: /^Verified email$/ }).check();
  await expect(page.getByLabel("Expected payer email")).toHaveValue("ap@initrode.example");
  await page.getByLabel("Expected payer email").fill("peter@initrode.example");
  await page.getByRole("button", { name: "Continue" }).click();

  // The review carries the whole request, attachment included.
  const summary = page.getByLabel("Request summary");
  await expect(summary).toContainText("retainer.pdf");
  await expect(summary).toContainText("Verified email");

  const created = page.waitForRequest(
    (request) => request.method() === "POST" && request.url().endsWith("/v1/deposit-requests"),
  );
  await page.getByRole("button", { name: "Issue deposit request" }).click();
  const body = (await created).postDataJSON();
  // The moment is sent as a moment; a duration would re-anchor it to arrival.
  expect(body.expires_at).toBe(new Date(chosen).toISOString());
  expect(body).not.toHaveProperty("expires_in");
  expect(body.notes).toBe("Net 15.");
  expect(body.attachment_id).toBeTruthy();
  // Settles to the account's own wallet, never typed by anyone.
  const wallet = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("payday.privy-stub.session") ?? "{}").wallet,
  );
  expect(body.payout_address).toBe(wallet);

  await expect(page.getByRole("heading", { name: "Deposit request issued." })).toBeVisible();
  // Tracking it lands back on the list with that request's row already open;
  // there is no page of its own to navigate to.
  await page.getByRole("button", { name: "Track this request" }).click();
  await expect(page).toHaveURL(/\/dashboard(\?.*)?$/);

  // The open row carries the whole request: deposit, verification, and files.
  await expect(page.getByRole("button", { name: /Design retainer/, expanded: true })).toBeVisible();
  await expect(page.getByText("Awaiting deposit").first()).toBeVisible();
  await expect(page.getByRole("progressbar", { name: "Received" })).toBeVisible();
  await expect(page.getByText("INV-2001").first()).toBeVisible();
  await expect(page.getByText("Net 15.")).toBeVisible();
  await expect(page.getByText("retainer.pdf")).toBeVisible();
  await expect(page.getByText("Verified email").first()).toBeVisible();
  await expect(page.getByText("peter@initrode.example")).toBeVisible();
  await expect(page.getByText("Pending").first()).toBeVisible();
  // No attempt yet, so nothing is broken out under the verdict.
  await expect(page.getByLabel("Verification activity")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Proof of Payment" })).toBeDisabled();
  await expect(page.getByText(/generated once the request settles/)).toBeVisible();
  await expect(page.getByText("Recovered funds")).toHaveCount(0);

  // The signed download URL is fetched on demand, not embedded.
  await page.addInitScript(() => {
    (window as unknown as { __opened: string[] }).__opened = [];
    window.open = (url) => {
      (window as unknown as { __opened: string[] }).__opened.push(String(url));
      return window;
    };
  });
  await page.reload();
  await page.getByRole("button", { name: /Design retainer/ }).click();
  await page.getByRole("button", { name: "Download attachment" }).click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened: string[] }).__opened))
    .toEqual([expect.stringContaining(`${STUB}/__download/`)]);

  // And its summary row still carries what the list is scanned for.
  // The summary row, not the detail row under it that repeats the title.
  const row = page.getByRole("row").filter({ hasText: "Design retainer" }).first();
  await expect(row).toContainText(customerName);
  await expect(row).toContainText("Verified email");
  await expect(row.getByLabel("Has attachment")).toBeVisible();
});

test("a settled invoice offers its PDF, its Proof of Payment, and its recovered funds", async ({
  page,
}) => {
  await signIn(page);
  await page.getByRole("button", { name: /Consulting — August/ }).click();

  await expect(page.getByText("Settled").first()).toBeVisible();
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
  expect(body.version).toBe("payday.proof.v2");
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

test("the account section shows the signed-in mailbox and the Payday wallet", async ({
  page,
}) => {
  await signIn(page, "account-view@example.com");

  const section = page.getByRole("region", { name: "Account" });
  await expect(section).toContainText("account-view@example.com");
  // The wallet is the account's own, shown in full and ready to copy; there
  // is no chain behind the stub, so the balance says so rather than spinning.
  const wallet = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("payday.privy-stub.session") ?? "{}").wallet,
  );
  expect(wallet).toMatch(/^0x[0-9a-f]{40}$/);
  await expect(section).toContainText(wallet);
  await expect(section.getByRole("button", { name: /Copy wallet address/ })).toBeVisible();
  await expect(section.getByText(/Balance unavailable/)).toBeVisible({ timeout: 20_000 });
});

test("a merchant can generate, roll, and revoke their API key from the session", async ({
  page,
}) => {
  await signIn(page, "api-key-flow@example.com");

  const section = page.getByRole("region", { name: "API key" });
  await expect(section).toContainText("No key yet");

  // A first key: one confirmation, no second sign-in.
  await section.getByRole("button", { name: "Generate key" }).click();
  await expect(section.getByText(/shown once, here, and never again/)).toBeVisible();
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
  await expect(
    section.getByText(/previous key keeps working for the next 24 hours/),
  ).toBeVisible();
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
