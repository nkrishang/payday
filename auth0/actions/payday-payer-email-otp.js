const crypto = require("crypto");

const CLAIM_NAMESPACE = "https://api.payday.sh/auth";
const MAX_AUTHENTICATION_AGE_SECONDS = 5 * 60;

/**
 * A payer proves mailbox ownership for one deposit request, and gatewayd exchanges the
 * OTP on the payer's behalf against a dedicated audience. This Action guards
 * only that audience: the merchant Action (`payday-email-otp.js`) never sees
 * it, and this one never sees the merchant API, so neither client can pick up
 * the other's claims. An unset secret is treated as "no such audience" so the
 * Action is inert until the payer resource server exists, and
 * `undefined === undefined` cannot admit a request that names no audience.
 */
function configuredSecret(secrets, name) {
  const value = secrets?.[name];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

exports.onExecutePostLogin = async (event, api) => {
  const payerAudience = configuredSecret(event.secrets, "PAYDAY_PAYER_AUDIENCE");
  if (
    payerAudience === undefined ||
    event.resource_server?.identifier !== payerAudience
  ) {
    return;
  }

  const payerClientId = configuredSecret(event.secrets, "PAYDAY_PAYER_CLIENT_ID");
  const emailMethod = event.authentication?.methods?.find(
    (method) => method.name === "email",
  );
  const authenticatedAt = Math.floor(
    new Date(emailMethod?.timestamp ?? "invalid").getTime() / 1000,
  );
  const now = Math.floor(Date.now() / 1000);
  const clientId = event.client?.client_id;
  const isPayerEmailOtp =
    payerClientId !== undefined &&
    clientId === payerClientId &&
    event.connection?.strategy === "email" &&
    event.user?.email_verified === true &&
    typeof event.user?.email === "string" &&
    Number.isFinite(authenticatedAt) &&
    authenticatedAt <= now + 30 &&
    now - authenticatedAt <= MAX_AUTHENTICATION_AGE_SECONDS;

  if (!isPayerEmailOtp) {
    api.access.deny("Payday payer verification requires a fresh email OTP.");
    return;
  }

  // gatewayd compares this claim with the merchant-supplied expected email,
  // so both sides must normalize the same way: trimmed and lowercased.
  const email = event.user.email.trim().toLowerCase();

  api.accessToken.setCustomClaim(`${CLAIM_NAMESPACE}/method`, "email_otp");
  api.accessToken.setCustomClaim(`${CLAIM_NAMESPACE}/client_id`, clientId);
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/authenticated_at`,
    authenticatedAt,
  );
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/event_id`,
    crypto.randomUUID(),
  );
  api.accessToken.setCustomClaim(`${CLAIM_NAMESPACE}/email`, email);
};
