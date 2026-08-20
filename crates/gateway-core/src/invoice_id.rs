use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvoiceId(pub Uuid);

pub fn generate_invoice_id() -> InvoiceId {
    InvoiceId(Uuid::now_v7())
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use uuid::Version;

    #[test]
    fn two_ids_differ() {
        let a = generate_invoice_id();
        let b = generate_invoice_id();

        assert_ne!(a, b);
    }

    #[test]
    fn ids_are_version_7() {
        let id = generate_invoice_id();
        assert_eq!(id.0.get_version(), Some(Version::SortRand));
    }

    #[test]
    fn ids_are_chronologically_sortable() {
        // The whole point of UUIDv7 over v4: later timestamps sort after
        // earlier ones. Sleep a few ms to guarantee a timestamp boundary.
        let a = generate_invoice_id();
        thread::sleep(Duration::from_millis(5));
        let b = generate_invoice_id();

        assert!(a.0 < b.0, "earlier ID should sort before later ID");
    }
}