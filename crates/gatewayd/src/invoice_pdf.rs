//! Deterministic invoice summary PDF (guide §Slice 2.6).
//!
//! The same invoice must render to byte-identical output so a PDF fetched
//! later still matches one exported at issuance: only base-14 Helvetica (no
//! embedded fonts), objects written in a fixed order, no creation date,
//! producer, or file identifier, and every value printed exactly as the API
//! already formats it (decimal amount strings, RFC 3339 timestamps). Only
//! issuance-time fields appear; nothing that changes as the payment progresses.

use gateway_core::{PayerPolicyMode, PaymentResponse};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};
use thiserror::Error;

/// A4, in points: paper that presumes no jurisdiction.
const PAGE_WIDTH: f32 = 595.0;
const PAGE_HEIGHT: f32 = 842.0;
const MARGIN: f32 = 56.0;
const TEXT_WIDTH: f32 = PAGE_WIDTH - 2.0 * MARGIN;
/// Where a field's value starts, leaving room for its label.
const VALUE_INDENT: f32 = 120.0;
const LINE_HEIGHT: f32 = 1.4;
const MAX_PAGES: usize = 32;

#[derive(Debug, Error)]
pub enum InvoicePdfError {
    #[error("the invoice needs more than {MAX_PAGES} pages")]
    TooManyPages,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
}

impl Font {
    fn resource(self) -> Name<'static> {
        match self {
            Font::Regular => Name(b"F1"),
            Font::Bold => Name(b"F2"),
        }
    }
}

/// One run of text at a horizontal offset from the margin.
struct Run {
    x: f32,
    font: Font,
    size: f32,
    text: Vec<u8>,
}

/// One typeset line: its runs and the vertical space it occupies.
struct Line {
    runs: Vec<Run>,
    height: f32,
}

/// Lay the invoice out as lines, then paginate and write them.
pub fn render_invoice_pdf(invoice: &PaymentResponse) -> Result<Vec<u8>, InvoicePdfError> {
    let lines = layout(invoice);
    let pages = paginate(&lines);
    if pages.len() > MAX_PAGES {
        return Err(InvoicePdfError::TooManyPages);
    }

    let catalog_id = Ref::new(1);
    let pages_id = Ref::new(2);
    let regular_id = Ref::new(3);
    let bold_id = Ref::new(4);
    let page_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(5 + 2 * index as i32))
        .collect();
    let content_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(6 + 2 * index as i32))
        .collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(pages_id);
    pdf.pages(pages_id)
        .kids(page_ids.iter().copied())
        .count(pages.len() as i32);
    pdf.type1_font(regular_id)
        .base_font(Name(b"Helvetica"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.type1_font(bold_id)
        .base_font(Name(b"Helvetica-Bold"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    for (index, page_lines) in pages.iter().enumerate() {
        let mut page = pdf.page(page_ids[index]);
        page.parent(pages_id)
            .media_box(Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT))
            .contents(content_ids[index]);
        page.resources()
            .fonts()
            .pair(Font::Regular.resource(), regular_id)
            .pair(Font::Bold.resource(), bold_id);
        page.finish();

        let mut content = Content::new();
        let mut y = PAGE_HEIGHT - MARGIN;
        for line in page_lines {
            y -= line.height;
            for run in &line.runs {
                content.begin_text();
                content.set_font(run.font.resource(), run.size);
                content.next_line(MARGIN + run.x, y);
                content.show(Str(&run.text));
                content.end_text();
            }
        }
        let bytes = content.finish();
        pdf.stream(content_ids[index], bytes.as_slice());
    }
    Ok(pdf.finish())
}

fn layout(invoice: &PaymentResponse) -> Vec<Line> {
    let mut lines = Lines::default();
    lines.text(Font::Bold, 18.0, "Invoice");
    if let Some(heading) = &invoice.heading {
        lines.text(Font::Regular, 12.0, heading);
    }
    lines.gap(8.0);
    lines.field("Payment ID", &invoice.id);
    if let Some(reference) = &invoice.reference {
        lines.field("Reference", reference);
    }
    lines.field("Issued", &invoice.created_at);
    lines.field("Payment due by", &invoice.expires_at);
    lines.field(
        "Amount",
        &format!("{} {}", invoice.amount, invoice.currency),
    );
    lines.field("Payer policy", policy_words(invoice.payer_policy.mode()));

    lines.section("From");
    lines.party(&invoice.issuer);
    lines.section("Bill to");
    lines.party(&invoice.bill_to);

    if let Some(notes) = &invoice.notes {
        lines.section("Notes");
        lines.text(Font::Regular, 10.0, notes);
    }

    lines.section("Payment");
    lines.field(
        "Chain",
        &format!("{} ({})", invoice.chain.name, invoice.chain.id),
    );
    lines.field(
        "Token",
        &format!("{} {}", invoice.token.symbol, invoice.token.address),
    );
    lines.field("Payment address", &invoice.address);

    if let Some(attachment) = &invoice.attachment {
        lines.section("Attachment");
        lines.field("Filename", &attachment.filename);
        lines.field("Size", &format!("{} bytes", attachment.byte_length));
        lines.field("SHA-256", &attachment.sha256);
    }

    lines.section("Attribution");
    lines.field("Version", &invoice.attribution.version.to_string());
    lines.field("Hash", &invoice.attribution.hash);
    lines.text(
        Font::Regular,
        8.0,
        "The payment address commits to this invoice: the hash above, a random nonce, and the \
         derived salt reproduce it offline from the Proof of Payment.",
    );
    lines.finish()
}

/// The mode in words; the assertions behind it stay out of the document.
fn policy_words(mode: PayerPolicyMode) -> &'static str {
    match mode {
        PayerPolicyMode::Permissionless => "Anyone holding the payment link may pay",
        PayerPolicyMode::VerifiedEmail => "The payer must verify their email address before paying",
        PayerPolicyMode::MerchantSession => {
            "The issuer's application opens this payment for its signed-in customer"
        }
    }
}

#[derive(Default)]
struct Lines {
    lines: Vec<Line>,
    pending_gap: f32,
}

impl Lines {
    fn gap(&mut self, points: f32) {
        self.pending_gap += points;
    }

    fn push(&mut self, runs: Vec<Run>, size: f32) {
        self.lines.push(Line {
            runs,
            height: size * LINE_HEIGHT + self.pending_gap,
        });
        self.pending_gap = 0.0;
    }

    /// Wrapped paragraphs of one style across the full text width.
    fn text(&mut self, font: Font, size: f32, text: &str) {
        for encoded in wrap(text, size, TEXT_WIDTH) {
            self.push(
                vec![Run {
                    x: 0.0,
                    font,
                    size,
                    text: encoded,
                }],
                size,
            );
        }
    }

    fn section(&mut self, title: &str) {
        self.gap(10.0);
        self.text(Font::Bold, 11.0, title);
    }

    /// A bold label with its value wrapped beside it; continuation lines
    /// stay aligned under the value.
    fn field(&mut self, label: &str, value: &str) {
        let size = 10.0;
        let mut first = true;
        for encoded in wrap(value, size, TEXT_WIDTH - VALUE_INDENT) {
            let mut runs = Vec::new();
            if first {
                runs.push(Run {
                    x: 0.0,
                    font: Font::Bold,
                    size,
                    text: encode(label),
                });
                first = false;
            }
            runs.push(Run {
                x: VALUE_INDENT,
                font: Font::Regular,
                size,
                text: encoded,
            });
            self.push(runs, size);
        }
    }

    fn party(&mut self, party: &gateway_core::Party) {
        self.text(Font::Regular, 10.0, &party.name);
        if let Some(email) = &party.email {
            self.text(Font::Regular, 10.0, email);
        }
        if let Some(details) = &party.details {
            self.text(Font::Regular, 10.0, details);
        }
    }

    fn finish(self) -> Vec<Line> {
        self.lines
    }
}

fn paginate(lines: &[Line]) -> Vec<Vec<&Line>> {
    let mut pages: Vec<Vec<&Line>> = vec![Vec::new()];
    let mut y = PAGE_HEIGHT - MARGIN;
    for line in lines {
        if y - line.height < MARGIN && !pages.last().is_some_and(Vec::is_empty) {
            pages.push(Vec::new());
            y = PAGE_HEIGHT - MARGIN;
        }
        y -= line.height;
        pages.last_mut().expect("one page exists").push(line);
    }
    pages
}

/// Greedy word wrap on measured Helvetica widths. Paragraph breaks are kept;
/// a word wider than the line is split by character.
fn wrap(text: &str, size: f32, width: f32) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current: Vec<u8> = Vec::new();
        for word in paragraph.split_whitespace() {
            let word = encode(word);
            let candidate_width = if current.is_empty() {
                measure(&word, size)
            } else {
                measure(&current, size) + measure(b" ", size) + measure(&word, size)
            };
            if candidate_width <= width {
                if !current.is_empty() {
                    current.push(b' ');
                }
                current.extend_from_slice(&word);
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            for byte in word {
                if measure(&current, size) + measure(&[byte], size) > width && !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                current.push(byte);
            }
        }
        lines.push(current);
    }
    lines
}

fn measure(text: &[u8], size: f32) -> f32 {
    text.iter().map(|&byte| glyph_width(byte)).sum::<u32>() as f32 * size / 1000.0
}

/// Helvetica advance widths (AFM, 1/1000 em) for printable ASCII; every
/// other WinAnsi glyph is taken as an average width, which only affects
/// wrapping, never determinism.
fn glyph_width(byte: u8) -> u32 {
    const WIDTHS: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ];
    match byte {
        b' '..=b'~' => u32::from(WIDTHS[usize::from(byte - b' ')]),
        _ => 556,
    }
}

/// WinAnsiEncoding: ASCII and Latin-1 map directly, the common typographic
/// characters occupy 0x80–0x9F, and anything else becomes `?` rather than a
/// glyph Helvetica cannot draw.
fn encode(text: &str) -> Vec<u8> {
    text.chars()
        .filter(|&c| c != '\r')
        .map(|c| match c {
            ' '..='~' | '\u{a0}'..='\u{ff}' => c as u8,
            '\t' => b' ',
            '\u{20ac}' => 0x80,
            '\u{201a}' => 0x82,
            '\u{0192}' => 0x83,
            '\u{201e}' => 0x84,
            '\u{2026}' => 0x85,
            '\u{2020}' => 0x86,
            '\u{2021}' => 0x87,
            '\u{02c6}' => 0x88,
            '\u{2030}' => 0x89,
            '\u{0160}' => 0x8a,
            '\u{2039}' => 0x8b,
            '\u{0152}' => 0x8c,
            '\u{017d}' => 0x8e,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{201c}' => 0x93,
            '\u{201d}' => 0x94,
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            '\u{02dc}' => 0x98,
            '\u{2122}' => 0x99,
            '\u{0161}' => 0x9a,
            '\u{203a}' => 0x9b,
            '\u{0153}' => 0x9c,
            '\u{017e}' => 0x9e,
            '\u{0178}' => 0x9f,
            _ => b'?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{U256, address};
    use gateway_core::{
        Amount, AttachmentDescriptor, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId,
        FactoryAddress, Invoice, Party, PayerPolicy, RecoveryAddress, TokenAddress,
    };
    use uuid::Uuid;

    use super::*;

    fn invoice(notes: Option<&str>) -> PaymentResponse {
        let factory = FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"));
        let token = TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(1_500_000));
        let recovery = RecoveryAddress(address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            Party {
                name: "Acme Corp".into(),
                email: Some("billing@acme.example".into()),
                details: Some("1 Main St\nSpringfield".into()),
            },
            Party {
                name: "Globex".into(),
                email: None,
                details: None,
            },
            PayerPolicy::VerifiedEmail {
                expected_email: "alice@example.com".into(),
            },
            factory,
            ChainId(143),
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
        );
        snapshot.heading = Some("March retainer — “final”".into());
        snapshot.reference = Some("INV-42".into());
        snapshot.notes = notes.map(str::to_owned);
        let invoice = Invoice::issue(
            factory,
            ChainId(143),
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
            snapshot,
        )
        .unwrap();
        let mut response = PaymentResponse::from_invoice(invoice, None);
        response.created_at = "2026-09-01T12:00:00+00:00".into();
        response.attachment = Some(AttachmentDescriptor {
            id: Uuid::from_u128(9),
            filename: "contract.pdf".into(),
            mime_type: "application/pdf".into(),
            byte_length: "48211".into(),
            sha256: format!("0x{}", "9f".repeat(32)),
            download_url: Some("https://signed.example/never-rendered".into()),
        });
        response
    }

    #[test]
    fn same_invoice_produces_identical_pdf_bytes() {
        let sample = invoice(Some("Net 30. Thank you."));
        let first = render_invoice_pdf(&sample).unwrap();
        let second = render_invoice_pdf(&sample).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(b"%PDF-"));
        assert!(first.trim_ascii_end().ends_with(b"%%EOF"));

        let text = String::from_utf8_lossy(&first);
        for absent in ["CreationDate", "ModDate", "Producer", "/ID", "FontFile"] {
            assert!(!text.contains(absent), "{absent} would vary or embed");
        }
        for present in [
            "Helvetica",
            "WinAnsiEncoding",
            "(Acme Corp)",
            "(INV-42)",
            "(1.500000 USDC)",
            "(contract.pdf)",
            "2026-09-01T12:00:00+00:00",
        ] {
            assert!(text.contains(present), "{present} missing");
        }
        assert!(
            !text.contains("alice@example.com"),
            "the policy's assertion stays out of the document"
        );
        assert!(!text.contains("signed.example"));

        // Different content renders differently, and long text paginates.
        let other = render_invoice_pdf(&invoice(Some("Net 60."))).unwrap();
        assert_ne!(first, other);
        let long = render_invoice_pdf(&invoice(Some(&"word ".repeat(1_500)))).unwrap();
        assert!(String::from_utf8_lossy(&long).contains("/Count 3"));
    }

    #[test]
    fn text_wraps_on_measured_widths_and_encodes_to_winansi() {
        let lines = wrap(&"word ".repeat(40), 10.0, TEXT_WIDTH);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| measure(line, 10.0) <= TEXT_WIDTH));
        let unbroken = wrap(&"x".repeat(300), 10.0, TEXT_WIDTH);
        assert!(unbroken.len() > 1, "a single overlong word is split");
        assert_eq!(wrap("a\n\nb", 10.0, TEXT_WIDTH).len(), 3);

        assert_eq!(
            encode("a–b€“q”"),
            vec![b'a', 0x96, b'b', 0x80, 0x93, b'q', 0x94]
        );
        assert_eq!(encode("é\tz\r"), vec![0xe9, b' ', b'z']);
        assert_eq!(encode("日本"), b"??");
    }
}
