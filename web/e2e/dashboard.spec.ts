import { expect, test, type Page } from "@playwright/test";

/**
 * The dashboard against e2e/stub-api.mjs, which plays the merchant API, the
 * presigned upload target, and the OTP issuer. Each test signs in on its own:
 * the session lives in the tab, and the stub issuer accepts one code.
 */

const STUB = "http://127.0.0.1:4010";
const OTP = "123456";
const PAYOUT = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const PDF = Buffer.from(
  "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n",
);

async function signIn(page: Page) {
  await page.goto("/dashboard/login");
  await page.getByLabel("Email").fill("merchant@example.com");
  await page.getByRole("button", { name: "Send code" }).click();
  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/dashboard\/invoices$/);
}

test("signing in takes an emailed code and rejects a wrong one", async ({ page }) => {
  await page.goto("/dashboard/login");
  await page.getByLabel("Email").fill("merchant@example.com");
  await page.getByRole("button", { name: "Send code" }).click();
  await expect(page.getByText(/We sent a code to merchant@example.com/)).toBeVisible();

  await page.getByLabel("One-time code").fill("000000");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText(/code is not valid/)).toBeVisible();

  await page.getByLabel("One-time code").fill(OTP);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/dashboard\/invoices$/);
  await expect(page.getByRole("heading", { name: "Invoices" })).toBeVisible();

  // The token is the session: in this tab only, never in localStorage.
  const storage = await page.evaluate(() => ({
    session: sessionStorage.getItem("payday.dashboard.session"),
    local: localStorage.length,
  }));
  expect(storage.session).toContain("stub-dashboard-token");
  expect(storage.local).toBe(0);
});

test("a signed-out visitor is sent to the login page and the shell carries no data", async ({
  page,
  request,
}) => {
  const response = await request.get("/dashboard/invoices");
  const html = await response.text();
  expect(response.status()).toBe(200);
  expect(html).toContain('name="robots"');
  expect(html).toContain("noindex");
  // Seeded invoices exist at the API; none of them is in the HTML.
  expect(html).not.toContain("Consulting — August");
  expect(html).not.toContain("Globex");
  expect(html).not.toContain("stub-dashboard-token");

  await page.goto("/dashboard/invoices");
  await expect(page).toHaveURL(/\/dashboard\/login$/);
});

test("the invoice list shows verification separately from payment and flags unsolicited funds", async ({
  page,
}) => {
  await signIn(page);

  const settled = page.getByRole("row").filter({ hasText: "Consulting — August" });
  await expect(settled).toContainText("Globex Corporation");
  await expect(settled).toContainText("25.00 USDC");
  await expect(settled).toContainText("Settled");
  await expect(settled).toContainText("Verified email");
  await expect(settled).toContainText("Verified");
  await expect(settled.getByLabel("Has attachment")).toBeVisible();

  const unsolicited = page.getByRole("row").filter({ hasText: "Retainer — September" });
  await expect(unsolicited).toContainText("Received");
  await expect(unsolicited).toContainText("Pending");
  await expect(unsolicited).toContainText("Likely unsolicited");

  // The filter is the API's status parameter, not a client-side sieve.
  await page.getByLabel("Status").selectOption("settled");
  await expect(page.getByRole("row").filter({ hasText: "Retainer — September" })).toHaveCount(0);
  await expect(page.getByRole("row").filter({ hasText: "Consulting — August" })).toHaveCount(1);
});

test("a merchant can create a customer, upload a PDF, issue an invoice, and open it", async ({
  page,
}) => {
  await signIn(page);

  // Customer.
  await page.getByRole("link", { name: "Customers" }).click();
  await page.getByRole("link", { name: "New customer" }).click();
  const customerName = `Initrode ${Date.now()}`;
  await page.getByLabel("Name").fill(customerName);
  await page.getByLabel("Email").fill("ap@initrode.example");
  await page.getByLabel("Details").fill("Vendor #4471");
  await page.getByRole("button", { name: "Create customer" }).click();
  await expect(page).toHaveURL(/\/dashboard\/customers\/[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { name: customerName })).toBeVisible();

  // Straight into an invoice for that customer.
  await page.getByRole("link", { name: "New invoice for this customer" }).click();
  await expect(page.getByLabel("Bill to name")).toHaveValue(customerName);
  await expect(page.getByLabel("Bill to email")).toHaveValue("ap@initrode.example");

  await page.getByLabel("Issuer name").fill("Acme Corp");
  await page.getByLabel("Amount (USDC)").fill("120.50");
  await page.getByLabel("Payout address").fill(PAYOUT);
  await page.getByLabel("Heading").fill("Design retainer");
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

  // A gated policy with its assertion.
  await page.getByRole("radio", { name: /^Verified identity$/ }).check();
  await page.getByLabel("Expected payer email").fill("peter@initrode.example");
  await page.getByLabel("Expected first name").fill("Peter");
  await page.getByLabel("Expected last name").fill("Gibbons");

  await page.getByRole("button", { name: "Issue invoice" }).click();
  await expect(page).toHaveURL(/\/dashboard\/invoices\/pay_[0-9a-f-]{36}$/);

  // Detail: document, payment, and verification are all there and separate.
  await expect(page.getByRole("heading", { name: "Design retainer" })).toBeVisible();
  await expect(page.getByText("Awaiting payment")).toBeVisible();
  await expect(page.getByText("120.50 USDC").first()).toBeVisible();
  await expect(page.getByText("INV-2001")).toBeVisible();
  await expect(page.getByText("Net 15.")).toBeVisible();
  await expect(page.getByText("retainer.pdf")).toBeVisible();
  await expect(page.getByText("Verified identity")).toBeVisible();
  await expect(page.getByText("peter@initrode.example")).toBeVisible();
  await expect(page.getByText("Peter Gibbons")).toBeVisible();
  await expect(page.getByText("Pending").first()).toBeVisible();
  await expect(page.getByText(/has not started verifying yet/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Proof of Payment" })).toBeDisabled();
  await expect(page.getByText(/generated once the invoice settles/)).toBeVisible();
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
  await page.getByRole("button", { name: "Download attachment" }).click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened: string[] }).__opened))
    .toEqual([expect.stringContaining(`${STUB}/__download/`)]);

  // And it is back in the list, linked to its customer.
  await page.getByRole("link", { name: "Invoices" }).click();
  const row = page.getByRole("row").filter({ hasText: "Design retainer" });
  await expect(row).toContainText(customerName);
  await expect(row).toContainText("Verified identity");
  await expect(row.getByLabel("Has attachment")).toBeVisible();
});

test("a settled invoice offers its PDF, its Proof of Payment, and its recovered funds", async ({
  page,
}) => {
  await signIn(page);
  await page.getByRole("link", { name: "Consulting — August" }).click();
  await expect(page).toHaveURL(/\/dashboard\/invoices\/pay_seed-settled$/);

  await expect(page.getByText("Settled").first()).toBeVisible();
  await expect(page.getByText("Verified", { exact: true })).toBeVisible();
  await expect(page.getByText("alice@globex.example")).toBeVisible();

  // Both the overpayment remainder and the late transfer went to recovery.
  const recovered = page.getByRole("region", { name: "Recovered funds" });
  await expect(recovered).toContainText("Overpayment remainder");
  await expect(recovered).toContainText("5.00 USDC");
  await expect(recovered).toContainText("Late transfer");
  await expect(recovered).toContainText("collected");

  const proofDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Proof of Payment" }).click();
  const proof = await proofDownload;
  expect(proof.suggestedFilename()).toBe("INV-1042-proof.json");
  const body = JSON.parse((await streamToString(proof)) ?? "");
  expect(body.version).toBe("payday.proof.v1");
  expect(body.payment_id).toBe("pay_seed-settled");
  expect(body.canonical_issuance_snapshot.attachment.sha256).toMatch(/^0x[0-9a-f]{64}$/);
  expect(body.verification.signer).toBe("0x976EA74026E726554dB657fA54763abd0C3a0aa9");

  const pdfDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Invoice PDF" }).click();
  const pdf = await pdfDownload;
  expect(pdf.suggestedFilename()).toBe("INV-1042.pdf");
  expect((await streamToString(pdf))?.startsWith("%PDF-")).toBe(true);
});

test("a declined identity check shows its facts, reference, and risk categories, and can be sent to review", async ({
  page,
}) => {
  await signIn(page);
  await page.getByRole("link", { name: "Retainer — September" }).click();
  await expect(page).toHaveURL(/\/dashboard\/invoices\/pay_seed-unsolicited$/);

  const activity = page.getByLabel("Verification activity");
  await expect(activity).toBeVisible();
  const facts = page.getByLabel("Verification facts");
  await expect(facts).toContainText("Email ownership");
  await expect(facts).toContainText("Identity document");
  await expect(facts).toContainText("Name matches the invoice");
  await expect(facts).toContainText("Declined");
  await expect(activity).toContainText("9f1c0f6e-1111-4c1a-9c1e-000000000003");
  await expect(activity).toContainText("EXPECTED_DETAILS_MISMATCH");
  await expect(activity).toContainText("ESP");
  await expect(activity).toContainText(/may try the identity check once more/);
  await expect(page.getByText("Likely unsolicited").first()).toBeVisible();
  const text = await page.locator("body").innerText();
  expect(text).not.toMatch(/SENTINEL|1900-01-01/);

  await page.getByRole("button", { name: "Request review" }).click();
  await expect(activity).toContainText(/awaiting a reviewer/);
  await expect(activity).toContainText("Review required");
  await expect(activity).not.toContainText(/may try the identity check once more/);
});

test("signing out ends the session", async ({ page }) => {
  await signIn(page);
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/dashboard\/login$/);
  expect(await page.evaluate(() => sessionStorage.getItem("payday.dashboard.session"))).toBeNull();
});

async function streamToString(download: {
  createReadStream: () => Promise<NodeJS.ReadableStream>;
}) {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(Buffer.from(chunk as Buffer));
  return Buffer.concat(chunks).toString("utf8");
}
