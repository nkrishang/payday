import { KeyRound, Lock, LockOpen, ShieldCheck } from "lucide-react";
import type { ReactNode } from "react";
import type { Auth, EndpointDoc, EndpointGroup, FieldDoc } from "./api/types";
import { CodeBlock } from "./code";
import { CodeTabs } from "./code-tabs";
import { Answers, MethodBadge, Params } from "./endpoint";
import { Pager } from "./pager";

/**
 * One route, one page: the reference's template. The left column reads top
 * to bottom — what the route does, what it takes, what it answers — and the
 * right column keeps the request and response samples in view beside it,
 * the way an engineer reads a reference: prose on one side, the shape of the
 * call on the other. Below `xl` the samples follow the summary instead.
 */

const AUTH: Record<Auth, { icon: typeof KeyRound; label: string; detail: string }> = {
  key: {
    icon: KeyRound,
    label: "API key",
    detail: "Authorization: Bearer payday_live_… — or the dashboard session.",
  },
  session: {
    icon: ShieldCheck,
    label: "Dashboard session only",
    detail: "An API key is refused here with 401 identity_unauthorized.",
  },
  none: { icon: LockOpen, label: "Public", detail: "No credential. Safe to call from a browser." },
  payer_session: {
    icon: Lock,
    label: "Payer session",
    detail: "Public; takes Payday-Payer-Session for content a policy gates.",
  },
};

export function EndpointPage({ group, endpoint }: { group: EndpointGroup; endpoint: EndpointDoc }) {
  const auth = AUTH[endpoint.auth];
  const AuthIcon = auth.icon;
  const samples = (
    <div className="flex flex-col gap-5">
      <div>
        <p className="mb-2 font-heading text-[11px] font-medium tracking-[0.08em] text-brand-grey uppercase">
          Request
        </p>
        {endpoint.examples.ts ? (
          <CodeTabs
            tabs={[
              { label: "curl", content: <CodeBlock code={endpoint.examples.curl} lang="bash" /> },
              { label: "TypeScript", content: <CodeBlock code={endpoint.examples.ts} lang="ts" /> },
            ]}
          />
        ) : (
          <CodeBlock code={endpoint.examples.curl} lang="bash" className="my-0" />
        )}
      </div>
      {endpoint.examples.response ? (
        <div>
          <p className="mb-2 font-heading text-[11px] font-medium tracking-[0.08em] text-brand-grey uppercase">
            Response
          </p>
          <CodeBlock
            code={endpoint.examples.response}
            lang={endpoint.examples.responseLang ?? "json"}
            title={endpoint.examples.responseTitle ?? "200 OK"}
            className="my-0"
          />
        </div>
      ) : null}
    </div>
  );

  return (
    <article className="docs-prose docs-endpoint-page">
      <div className="xl:grid xl:grid-cols-[minmax(0,1fr)_440px] xl:gap-10">
        <div className="min-w-0">
          <p className="mb-3 font-heading text-[12px] font-medium tracking-[0.08em] text-brand-green uppercase">
            {group.title}
          </p>
          <h1 className="font-heading text-[clamp(28px,3.4vw,36px)] leading-[1.1] font-medium tracking-[-0.04em] text-balance">
            {endpoint.title}
          </h1>

          <div className="mt-5 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-[10px] border border-line bg-surface px-3.5 py-2.5 font-mono text-[14px]">
            <MethodBadge method={endpoint.method} />
            <span className="text-ink break-all">{endpoint.path}</span>
          </div>

          <p className="mt-5 text-[16px] leading-[1.65] text-muted">{endpoint.summary}</p>

          <p className="mt-4 flex items-start gap-2 text-[13.5px] leading-[1.55] text-muted">
            <AuthIcon className="mt-0.5 size-3.5 shrink-0 text-faint" />
            <span>
              <span className="font-medium text-ink">{auth.label}.</span> {auth.detail}
            </span>
          </p>

          <div className="mt-6 xl:hidden">{samples}</div>

          {endpoint.body ? <div className="docs-endpoint-body mt-6">{endpoint.body}</div> : null}

          <FieldSection title="Headers" fields={endpoint.headers} />
          <FieldSection title="Path parameters" fields={endpoint.pathParams} />
          <FieldSection title="Query parameters" fields={endpoint.query} />
          <FieldSection title="Body" fields={endpoint.bodyFields} />

          {endpoint.response ? (
            <section className="mt-9">
              <SectionTitle>Response</SectionTitle>
              {endpoint.response.description ? (
                <p className="mt-2 text-[15px] leading-[1.65] text-muted">
                  {endpoint.response.description}
                </p>
              ) : null}
              {endpoint.response.fields ? <Params rows={endpoint.response.fields} /> : null}
            </section>
          ) : null}

          {endpoint.answers ? (
            <section className="mt-9">
              <SectionTitle>Answers</SectionTitle>
              <Answers rows={endpoint.answers} bare />
            </section>
          ) : null}

          <Pager />
        </div>

        <aside className="hidden xl:block">
          <div className="sticky top-[136px] max-h-[calc(100dvh-152px)] overflow-y-auto pr-1">
            {samples}
          </div>
        </aside>
      </div>
    </article>
  );
}

function SectionTitle({ children }: { children: ReactNode }) {
  return (
    <h2 className="font-heading text-[18px] font-medium tracking-[-0.02em] text-ink">{children}</h2>
  );
}

function FieldSection({ title, fields }: { title: string; fields: FieldDoc[] | undefined }) {
  if (!fields || fields.length === 0) return null;
  return (
    <section className="mt-9">
      <SectionTitle>{title}</SectionTitle>
      <Params rows={fields} />
    </section>
  );
}
