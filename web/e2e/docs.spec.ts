import { expect, test } from "@playwright/test";
import { DOCS_PAGES } from "../components/docs/nav";

/**
 * The documentation is static and reads nothing from the API, so what the
 * suite checks is that it is all there and holds together: every page in
 * the table of contents renders with its title, the landing page's Docs link
 * arrives here, the section list and the on-page outline are wired, the
 * sample tabs remember their choice, and nothing scrolls sideways on a
 * phone.
 */

test("the landing page's Docs link opens the documentation", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Docs" }).click();
  await expect(page).toHaveURL(/\/docs$/);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Stablecoin deposits");
});

test("every page in the table of contents renders", async ({ page }) => {
  for (const entry of DOCS_PAGES) {
    const response = await page.goto(entry.href);
    expect(response?.status(), entry.href).toBe(200);
    await expect(page.getByRole("heading", { level: 1 }), entry.href).toBeVisible();
    // By href, not title: "Webhooks" names both the guide and the reference page.
    await expect(
      page.getByRole("navigation", { name: "Documentation" }).locator(`a[href="${entry.href}"]`),
      entry.href,
    ).toHaveAttribute("aria-current", "page");
  }
});

test("the outline lists the page's own headings and the pager walks the list", async ({ page }) => {
  await page.goto("/docs/concepts");
  const outline = page.getByRole("navigation", { name: "On this page" });
  await expect(outline.getByRole("link", { name: "Lifecycle" })).toBeVisible();
  await expect(outline.getByRole("link", { name: "Where the USDC goes" })).toBeVisible();

  const pager = page.getByRole("navigation", { name: "Adjacent pages" });
  await expect(pager.getByRole("link", { name: /Quickstart/ })).toHaveAttribute(
    "href",
    "/docs/quickstart",
  );
  await pager.getByRole("link", { name: /Verifying the payer/ }).click();
  await expect(page).toHaveURL(/\/docs\/payer-verification$/);
});

test("choosing a sample language applies to every tab group and survives a reload", async ({
  page,
}) => {
  await page.goto("/docs/quickstart");
  const tabs = page.getByRole("tab", { name: "TypeScript" });
  await tabs.first().click();
  for (const tab of await tabs.all()) await expect(tab).toHaveAttribute("aria-selected", "true");
  await page.reload();
  await expect(page.getByRole("tab", { name: "TypeScript" }).first()).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("route headings in the reference carry deep links", async ({ page }) => {
  await page.goto("/docs/api/deposit-requests#post-deposit-requests-id-cancel");
  await expect(page.locator("#post-deposit-requests-id-cancel")).toBeInViewport();
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test("the section list is a drawer and nothing scrolls sideways", async ({ page }) => {
    for (const path of [
      "/docs",
      "/docs/concepts",
      "/docs/api/deposit-requests",
      "/docs/webhooks",
    ]) {
      await page.goto(path);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
      );
      expect(overflow, path).toBe(false);
    }

    await page.getByRole("button", { name: "Open navigation" }).click();
    await page
      .getByRole("navigation", { name: "Documentation" })
      .getByRole("link", { name: "Quickstart" })
      .click();
    await expect(page).toHaveURL(/\/docs\/quickstart$/);
    await expect(page.getByRole("button", { name: "Open navigation" })).toBeVisible();
  });
});
