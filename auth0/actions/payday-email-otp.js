const crypto = require("crypto");

const CLAIM_NAMESPACE = "https://api.payday.sh/auth";
const MAX_AUTHENTICATION_AGE_SECONDS = 5 * 60;

/**
 * The one first-party application allowed to hold a Payday API token: the
 * dashboard (SPA). It is configured as an Action secret so a tenant edit
 * cannot admit another client without a reviewed change. An unset secret is
 * dropped rather than compared, because `undefined === undefined` would
 * otherwise admit a request that carries no client at all.
 */
function allowedClientIds(secrets) {
  return [secrets.PAYDAY_CLIENT_ID].filter(
    (id) => typeof id === "string" && id.length > 0,
  );
}

exports.onExecutePostLogin = async (event, api) => {
  if (event.resource_server?.identifier !== event.secrets.PAYDAY_API_AUDIENCE) {
    return;
  }

  const emailMethod = event.authentication?.methods?.find(
    (method) => method.name === "email",
  );
  const authenticatedAt = Math.floor(
    new Date(emailMethod?.timestamp ?? "invalid").getTime() / 1000,
  );
  const now = Math.floor(Date.now() / 1000);
  const clientId = event.client?.client_id;
  const isPaydayEmailOtp =
    typeof clientId === "string" &&
    allowedClientIds(event.secrets).includes(clientId) &&
    event.connection?.strategy === "email" &&
    event.user?.email_verified === true &&
    typeof event.user?.email === "string" &&
    Number.isFinite(authenticatedAt) &&
    authenticatedAt <= now + 30 &&
    now - authenticatedAt <= MAX_AUTHENTICATION_AGE_SECONDS;

  if (!isPaydayEmailOtp) {
    api.access.deny("Payday account management requires a fresh email OTP.");
    return;
  }

  api.accessToken.setCustomClaim(`${CLAIM_NAMESPACE}/method`, "email_otp");
  api.accessToken.setCustomClaim(`${CLAIM_NAMESPACE}/email`, event.user.email);
  api.accessToken.setCustomClaim(`${CLAIM_NAMESPACE}/client_id`, clientId);
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/authenticated_at`,
    authenticatedAt,
  );
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/event_id`,
    crypto.randomUUID(),
  );
};
