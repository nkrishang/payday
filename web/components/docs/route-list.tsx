import Link from "next/link";
import type { EndpointGroup } from "./api/types";
import { MethodBadge } from "./endpoint";

/** The routes of one or more groups, as an overview page lists them. */
export function RouteList({ groups }: { groups: EndpointGroup[] }) {
  return (
    <ul className="docs-route-list">
      {groups.flatMap((group) =>
        group.endpoints.map((endpoint) => (
          <li key={`${group.slug}/${endpoint.slug}`} className="flex items-center gap-2.5">
            <MethodBadge method={endpoint.method} variant="tint" />
            <Link href={`/docs/api/${group.slug}/${endpoint.slug}`}>{endpoint.title}</Link>
            <span className="hidden font-mono text-[12px] text-faint sm:inline">
              {endpoint.path}
            </span>
          </li>
        )),
      )}
    </ul>
  );
}
