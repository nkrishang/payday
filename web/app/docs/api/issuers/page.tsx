import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { Callout, DocsPage, H2, Step, Steps } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Issuers and payout addresses API",
  description:
    "Save the party you issue under, prove its contact mailbox, and keep the wallets you settle to.",
};

const ISSUER = `{
  "id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
  "name": "Acme LLC",
  "contact_email": "billing@acme.example",
  "details": "Acme LLC, 1 Market St, Springfield",
  "email_verified": true,
  "email_verified_at": "2026-09-06T12:05:00Z",
  "payout_addresses": [
    { "id": "pa_0198f80c-…", "address": "0x1111111111111111111111111111111111111111", "label": "Treasury", "created_at": "2026-09-06T12:00:00Z" }
  ],
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:05:00Z"
}`;

const SETUP = `const acme = await payday.issuers.create({ name: "Acme LLC", contact_email: "billing@acme.example" });

const { contact_email, resend_available_at } = await payday.issuers.startEmailVerification(acme.id);
// … the code arrives at contact_email …
await payday.issuers.confirmEmailVerification(acme.id, "123456");

const treasury = await payday.payoutAddresses.create({ address: "0x1111…", label: "Treasury" });
await payday.issuers.setPayoutAddresses(acme.id, [treasury.id]);`;

export default function IssuersApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Issuers and payout addresses"
      lead="An issuer identity is your own side of a deposit request, saved once instead of retyped: the party it is issued under, the mailbox payers write to, and the wallets it settles to."
    >
      <p>
        Issuance is unchanged by any of this. A request still carries its own <code>issuer</code>{" "}
        and <code>payout_address</code> snapshot, so editing an identity never reaches a request
        already issued. What the identity does is stop the retyping, prove the mailbox, and leave a
        durable <code>issuer_id</code> on every request issued under it.
      </p>
      <CodeBlock code={ISSUER} lang="json" title="The issuer object" />

      <H2 id="setting-one-up">Setting one up</H2>
      <Steps>
        <Step title="Create it">
          A name and a contact address. Names are unique among your identities, ignoring case and
          surrounding space; contact addresses may repeat.
        </Step>
        <Step title="Prove the mailbox">
          Start verification and a six-digit code goes to the stored address; confirm it. Payers are
          told to write there, so a request cannot carry an unproven mailbox.
        </Step>
        <Step title="Save the wallets it settles to (optional)">
          Payout addresses are account-owned and shared between identities. An identity with none
          settles to the account&apos;s own Payday wallet.
        </Step>
      </Steps>
      <CodeBlock code={SETUP} lang="ts" />

      <H2 id="issuers">Issuers</H2>
      <Endpoint method="POST" path="/v1/issuers">
        Create an identity, unverified.
      </Endpoint>
      <Params
        title="Body"
        rows={[
          {
            name: "name",
            type: "string",
            required: true,
            description: "1 to 255 bytes, unique among your identities.",
          },
          {
            name: "contact_email",
            type: "string",
            required: true,
            description: "3 to 254 bytes, lowercased on write.",
          },
          {
            name: "details",
            type: "string",
            description: "Up to 4,000 bytes, shown verbatim on requests.",
          },
        ]}
      />
      <Answers
        rows={[
          {
            status: 409,
            code: "issuer_name_taken",
            when: "Another of your identities already uses this name.",
          },
        ]}
      />

      <Endpoint method="GET" path="/v1/issuers">
        List identities as <code>{`{ issuers, next_cursor }`}</code>; <code>limit</code> and{" "}
        <code>starting_after</code> as usual.
      </Endpoint>

      <Endpoint method="GET" path="/v1/issuers/{id}">
        Read one identity.
      </Endpoint>

      <Endpoint method="PATCH" path="/v1/issuers/{id}">
        Update any subset of <code>name</code>, <code>contact_email</code>, and <code>details</code>
        .
      </Endpoint>
      <p>
        Partial, like a customer. A rename alone never touches a proven mailbox; a different{" "}
        <code>contact_email</code> clears the verification, because a different mailbox is a
        different claim.
      </p>

      <Endpoint method="DELETE" path="/v1/issuers/{id}">
        Delete an identity and its payout-address associations. <code>204</code>.
      </Endpoint>
      <Answers
        rows={[
          {
            status: 409,
            code: "issuer_in_use",
            when: "Requests were issued under it. They keep pointing at it.",
          },
        ]}
      />

      <Endpoint method="POST" path="/v1/issuers/{id}/verify/email/start">
        Email a code to the stored contact address.
      </Endpoint>
      <p>
        The request names no mailbox. Answers{" "}
        <code>202 {`{ contact_email, resend_available_at }`}</code>.
      </p>
      <Answers
        rows={[
          {
            status: 429,
            code: "otp_resend_cooldown",
            when: "One code per identity per minute; see Retry-After.",
          },
          { status: 409, code: "issuer_email_already_verified", when: "Already proven." },
          {
            status: 503,
            code: "verification_unavailable",
            when: "The environment has no email verification configured.",
          },
        ]}
      />

      <Endpoint method="POST" path="/v1/issuers/{id}/verify/email/confirm">
        Confirm the code: <code>{`{ "otp": "123456" }`}</code>. Answers the issuer with{" "}
        <code>email_verified</code>.
      </Endpoint>
      <Answers
        rows={[
          {
            status: 401,
            code: "otp_invalid",
            when: "Wrong, spent, expired, or confirmed after the address moved.",
          },
        ]}
      />

      <Endpoint method="PUT" path="/v1/issuers/{id}/payout-addresses">
        Replace the whole set of wallets this identity may settle to.
      </Endpoint>
      <Params
        title="Body"
        rows={[
          {
            name: "payout_address_ids",
            type: "pa_ id[]",
            required: true,
            description:
              "At most 25, in the order they should be offered. The first is the default when a request names issuer_id without a payout address.",
          },
        ]}
      />
      <Answers
        rows={[{ status: 404, code: "payout_address_not_found", when: "An id that is not yours." }]}
      />

      <H2 id="payout-addresses">Payout addresses</H2>
      <Endpoint method="POST" path="/v1/payout-addresses">
        Save a wallet: <code>{`{ "address": "0x…", "label": "Treasury" }`}</code>.
      </Endpoint>
      <p>
        Stored EIP-55 checksummed and once per account; saving one you already hold returns the row
        you have. <code>label</code> is at most 20 characters, starting with a letter or digit,
        using letters, digits, spaces, and <code>. _ &apos; &amp; ( ) -</code>.
      </p>

      <Endpoint method="GET" path="/v1/payout-addresses">
        List saved wallets as <code>{`{ payout_addresses }`}</code>.
      </Endpoint>

      <Endpoint method="DELETE" path="/v1/payout-addresses/{id}">
        Remove a wallet and every association to it. <code>204</code>.
      </Endpoint>

      <Callout title="Where a request settles">
        <code>POST /v1/deposit-requests</code> with an <code>issuer_id</code> and no{" "}
        <code>payout_address</code> settles to the identity&apos;s first saved wallet. An explicit{" "}
        <code>payout_address</code> always wins. Before accepting deposits, confirm your
        organisation controls whichever wallet that is.
      </Callout>
    </DocsPage>
  );
}
