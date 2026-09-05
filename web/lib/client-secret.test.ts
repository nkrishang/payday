import { describe, expect, it } from "vitest";
import { clientSecretFromFragment, takeClientSecret } from "./client-secret";

const SECRET = "cs_" + "a".repeat(43);

describe("clientSecretFromFragment", () => {
  it("reads a well-formed secret from the fragment, with or without the hash", () => {
    expect(clientSecretFromFragment(`#cs=${SECRET}`)).toBe(SECRET);
    expect(clientSecretFromFragment(`cs=${SECRET}`)).toBe(SECRET);
    expect(clientSecretFromFragment(`#utm=1&cs=${SECRET}`)).toBe(SECRET);
  });

  it("ignores anything that is not a client secret", () => {
    expect(clientSecretFromFragment("")).toBeNull();
    expect(clientSecretFromFragment("#section")).toBeNull();
    expect(clientSecretFromFragment("#cs=")).toBeNull();
    expect(clientSecretFromFragment("#cs=pps_not-a-secret")).toBeNull();
    expect(clientSecretFromFragment(`#cs=${SECRET}x`)).toBeNull();
    expect(clientSecretFromFragment(`#cs=cs_${"a".repeat(42)}`)).toBeNull();
    expect(clientSecretFromFragment(`#secret=${SECRET}`)).toBeNull();
  });
});

describe("takeClientSecret", () => {
  it("returns the secret and removes the fragment from the address bar", () => {
    window.history.replaceState(null, "", `/pay/dr_1?x=1#cs=${SECRET}`);
    expect(takeClientSecret()).toBe(SECRET);
    expect(window.location.hash).toBe("");
    expect(window.location.pathname + window.location.search).toBe("/pay/dr_1?x=1");
    // Once taken, it is gone: a second look finds the bare link.
    expect(takeClientSecret()).toBeNull();
  });

  it("returns null for a bare link and leaves the URL alone", () => {
    window.history.replaceState(null, "", "/pay/dr_1");
    expect(takeClientSecret()).toBeNull();
    expect(window.location.pathname).toBe("/pay/dr_1");
  });

  it("strips a fragment that is not a secret too, and reports none", () => {
    window.history.replaceState(null, "", "/pay/dr_1#cs=garbage");
    expect(takeClientSecret()).toBeNull();
    expect(window.location.hash).toBe("");
  });
});
