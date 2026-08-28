const crypto = require("crypto");

const CLAIM_NAMESPACE = "https://api.payday.sh/auth";
const MAX_AUTHENTICATION_AGE_SECONDS = 5 * 60;

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
  const isPaydayEmailOtp =
    event.client?.client_id === event.secrets.PAYDAY_CLIENT_ID &&
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
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/client_id`,
    event.client.client_id,
  );
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/authenticated_at`,
    authenticatedAt,
  );
  api.accessToken.setCustomClaim(
    `${CLAIM_NAMESPACE}/event_id`,
    crypto.randomUUID(),
  );
};
