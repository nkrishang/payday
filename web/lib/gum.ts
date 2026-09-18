import { GumPayerClient } from "@gum/sdk";
import { config } from "./config";

/**
 * The checkout is built on the same public client Gum ships to merchants who
 * want to render their own. It needs no API key: a deposit link is open by
 * design, because anyone holding it is allowed to fund the deposit request.
 */
export const payerClient = new GumPayerClient({ baseUrl: config.apiUrl });
