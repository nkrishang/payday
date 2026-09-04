const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const template = fs.readFileSync(
  path.join(__dirname, "passwordless-code.liquid"),
  "utf8",
);
const terraform = fs.readFileSync(
  path.join(__dirname, "..", "passwordless.tf"),
  "utf8",
);

test("contains the complete, non-enumerating sign-in message", () => {
  for (const copy of [
    "Enter this one-time code in your terminal",
    "This code expires in 5 minutes",
    "If you didn't request this, you can safely ignore this email",
    "support@payday.sh",
  ]) {
    assert.match(template, new RegExp(copy.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }

  assert.doesNotMatch(template, /invoice|payment|account (?:exists|does not exist)/i);
});

test("renders only the OTP and does not load active remote content", () => {
  assert.deepEqual(template.match(/{{.*?}}|{%.*?%}/gs), ["{{ code }}"]);
  assert.doesNotMatch(template, /<(?:script|form|iframe)\b|(?:src|background)=["']https?:/i);
});

test("Terraform owns the branded template and its stated lifetime", () => {
  assert.match(terraform, /strategy\s*=\s*"email"/);
  assert.match(terraform, /syntax\s*=\s*"liquid"/);
  assert.match(terraform, /email\/passwordless-code\.liquid/);
  assert.match(terraform, /time_step\s*=\s*300/);
  assert.match(terraform, /length\s*=\s*6/);
  assert.match(terraform, /brute_force_protection\s*=\s*true/);
});
