import { PaydayPayerClient } from "@payday/sdk";
import { config } from "./config";

/**
 * The checkout is built on the same public client Payday ships to merchants who
 * want to render their own. It needs no API key: a payment link is open by
 * design, because anyone holding it is allowed to fulfil the payment.
 */
export const payerClient = new PaydayPayerClient({ baseUrl: config.apiUrl });
