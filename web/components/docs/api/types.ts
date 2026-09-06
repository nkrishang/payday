import type { ReactNode } from "react";

/**
 * One route of the HTTP API, as the reference documents it. Every endpoint
 * page, the reference sidebar, the pager, and the browser suite read these
 * records, so a route is described once and appears everywhere.
 */

export type Method = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

/** Which credential the route takes. */
export type Auth =
  | "key" // an API key, or the dashboard session
  | "session" // the dashboard session only; an API key is refused
  | "none" // public: the payer routes
  | "payer_session"; // public, but needs the Payday-Payer-Session header

export interface FieldDoc {
  name: string;
  type: string;
  required?: boolean;
  description: ReactNode;
}

export interface AnswerDoc {
  status: number;
  code?: string;
  when: ReactNode;
}

export interface EndpointDoc {
  /** The last path segment of the page: `/docs/api/<group>/<slug>`. */
  slug: string;
  /** The sidebar name: "Create a deposit request". */
  title: string;
  method: Method;
  path: string;
  auth: Auth;
  /** One or two sentences on what the route does, under the title. */
  summary: ReactNode;
  /** Longer prose after the summary, before the fields. */
  body?: ReactNode;
  headers?: FieldDoc[];
  pathParams?: FieldDoc[];
  query?: FieldDoc[];
  bodyFields?: FieldDoc[];
  response?: {
    description?: ReactNode;
    fields?: FieldDoc[];
  };
  answers?: AnswerDoc[];
  examples: {
    curl: string;
    ts?: string;
    /** The response sample, JSON unless `responseLang` says otherwise. */
    response?: string;
    responseTitle?: string;
    responseLang?: "json" | "http" | "text";
  };
}

export interface EndpointGroup {
  /** The path segment: `/docs/api/<slug>/…`. */
  slug: string;
  title: string;
  /** An overview page shown first in the group, without a method badge. */
  overview?: { title: string };
  endpoints: EndpointDoc[];
}

export function endpointHref(group: EndpointGroup, endpoint: EndpointDoc): string {
  return `/docs/api/${group.slug}/${endpoint.slug}`;
}
