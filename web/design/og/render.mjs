import { chromium } from "@playwright/test";
import { fileURLToPath } from "node:url";
import path from "node:path";

/** Renders design/og/index.html to app/opengraph-image.png. Run from web/. */
const here = path.dirname(fileURLToPath(import.meta.url));
const browser = await chromium.launch();
const page = await (
  await browser.newContext({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 })
).newPage();
await page.goto(`file://${path.join(here, "index.html")}`);
await page.evaluate(() => window.__ready);
await page.waitForTimeout(400);
await page.screenshot({ path: path.join(here, "../../app/opengraph-image.png"), type: "png" });
await browser.close();
console.log("wrote app/opengraph-image.png");
