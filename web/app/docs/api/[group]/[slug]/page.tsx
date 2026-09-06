import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { API_ENDPOINTS, findEndpoint } from "@/components/docs/api";
import { EndpointPage } from "@/components/docs/endpoint-page";

/**
 * Every endpoint of the reference, one page each, from the records in
 * `components/docs/api`. The list of routes is closed at build time, so an
 * unknown pair is a 404 rather than a fallback page.
 */

type Params = { group: string; slug: string };

export const dynamicParams = false;

export function generateStaticParams(): Params[] {
  return API_ENDPOINTS.map(({ group, endpoint }) => ({ group: group.slug, slug: endpoint.slug }));
}

export async function generateMetadata({ params }: { params: Promise<Params> }): Promise<Metadata> {
  const { group, slug } = await params;
  const ref = findEndpoint(group, slug);
  if (!ref) return {};
  const summary = typeof ref.endpoint.summary === "string" ? ref.endpoint.summary : undefined;
  return {
    title: `${ref.endpoint.title} · ${ref.group.title}`,
    ...(summary ? { description: summary } : {}),
  };
}

export default async function ApiEndpointPage({ params }: { params: Promise<Params> }) {
  const { group, slug } = await params;
  const ref = findEndpoint(group, slug);
  if (!ref) notFound();
  return <EndpointPage group={ref.group} endpoint={ref.endpoint} />;
}
