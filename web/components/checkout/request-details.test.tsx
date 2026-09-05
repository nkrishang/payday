import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { payment } from "@/test/fixtures";
import { RequestDetails } from "./request-details";

const ATTACHMENT = {
  id: "0198f80c-8d2f-7dc1-a369-90556a64f7aa",
  filename: "INV-1042.pdf",
  mime_type: "application/pdf" as const,
  byte_length: "48211",
  sha256: "0x9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
};

describe("RequestDetails", () => {
  it("renders the document: parties, reference, amount, notes, and the attachment", () => {
    render(
      <RequestDetails
        payment={payment({
          heading: "Consulting — August",
          details: {
            amount: "25.00",
            amount_base_units: "25000000",
            payer: { name: "Globex Corporation", email: "ap@globex.example", details: "PO 7781" },
            notes: "Net 30.\nThank you.",
            reference: "INV-1042",
            attachment: ATTACHMENT,
          },
        })}
      />,
    );

    expect(screen.getByRole("region", { name: "Deposit request" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Consulting — August" })).toBeInTheDocument();
    expect(screen.getByText("Acme Corp")).toBeInTheDocument();
    expect(screen.getByText("Globex Corporation")).toBeInTheDocument();
    expect(screen.getByText("ap@globex.example")).toBeInTheDocument();
    expect(screen.getByText("PO 7781")).toBeInTheDocument();
    expect(screen.getByText("INV-1042")).toBeInTheDocument();
    expect(screen.getByText("25.00 USDC")).toBeInTheDocument();
    // Notes keep their line breaks and are never interpreted.
    expect(screen.getByText(/Net 30\./)).toHaveClass("whitespace-pre-wrap");
    expect(screen.getByRole("button", { name: /INV-1042\.pdf/ })).toBeInTheDocument();
  });

  it("omits what the deposit request does not carry", () => {
    render(<RequestDetails payment={payment()} />);

    expect(screen.queryByRole("heading")).not.toBeInTheDocument();
    expect(screen.queryByText("Reference")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("25.00 USDC")).toBeInTheDocument();
  });
});
