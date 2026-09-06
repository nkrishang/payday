import { ACCOUNT_GROUP, STATUS_GROUP } from "./account";
import { ATTACHMENTS } from "./attachments";
import { CUSTOMERS } from "./customers";
import { DEPOSIT_REQUESTS } from "./deposit-requests";
import { ISSUERS, PAYOUT_ADDRESSES } from "./issuers";
import { PAYER } from "./payer";
import { type EndpointDoc, type EndpointGroup, endpointHref } from "./types";
import { WEBHOOKS } from "./webhooks";

/** Every group of the reference, in sidebar order. */
export const API_GROUPS: ReadonlyArray<EndpointGroup> = [
  DEPOSIT_REQUESTS,
  CUSTOMERS,
  ISSUERS,
  PAYOUT_ADDRESSES,
  ATTACHMENTS,
  WEBHOOKS,
  ACCOUNT_GROUP,
  STATUS_GROUP,
  PAYER,
];

export interface EndpointRef {
  group: EndpointGroup;
  endpoint: EndpointDoc;
  href: string;
}

export const API_ENDPOINTS: ReadonlyArray<EndpointRef> = API_GROUPS.flatMap((group) =>
  group.endpoints.map((endpoint) => ({ group, endpoint, href: endpointHref(group, endpoint) })),
);

export function findEndpoint(groupSlug: string, slug: string): EndpointRef | undefined {
  return API_ENDPOINTS.find((ref) => ref.group.slug === groupSlug && ref.endpoint.slug === slug);
}

export function isEndpointPath(pathname: string): boolean {
  return API_ENDPOINTS.some((ref) => ref.href === pathname);
}

export { endpointHref };
export type { EndpointDoc, EndpointGroup };
