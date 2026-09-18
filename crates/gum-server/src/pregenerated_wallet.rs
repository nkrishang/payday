//! Pregenerating a merchant's embedded wallet before they finish signing in.
//!
//! Privy's `createOnLogin` does not apply to the dashboard's headless email
//! login (`useLoginWithEmail`/`loginWithCode`) — Privy only ever creates a
//! wallet here because the sign-up dialog explicitly asks for one
//! (`useCreateWallet`) once the code is verified. Asking Privy's server API
//! to create that same wallet the moment a merchant submits their email,
//! in parallel with Privy sending the code, gives Privy the several seconds
//! it takes a person to open that email and type the code back to finish the
//! same work, so the client-side `useCreateWallet` call usually finds the
//! wallet already there instead of waiting on it.
//!
//! Best-effort only: sign-in's own explicit `useCreateWallet` call is the
//! correctness guarantee regardless of whether this ever runs, races, or
//! fails, so failures here are logged, not surfaced.

use std::time::Duration;

use async_trait::async_trait;

const PREGENERATE_URL: &str = "https://auth.privy.io/api/v1/users";

#[derive(Debug)]
pub enum PregenerateError {
    /// Privy could not be reached, or answered outside its documented
    /// contract. There is no "rejected" case: creating a user for an email
    /// that already has one is idempotent (verified against the live API —
    /// it returns the existing user and wallet unchanged, not an error).
    Unavailable(String),
}

/// Behind a trait so routes.rs's tests can stand in for Privy. This is not a
/// vendor abstraction: Privy is the only implementation that ships.
#[async_trait]
pub trait WalletPregenerator: Send + Sync {
    async fn pregenerate(&self, email: &str) -> Result<(), PregenerateError>;
}

pub struct PrivyPregeneration {
    http: reqwest::Client,
    app_id: String,
    app_secret: String,
}

impl PrivyPregeneration {
    pub fn new(app_id: String, app_secret: String) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("failed to build Privy HTTP client: {error}"))?;
        Ok(Self {
            http,
            app_id,
            app_secret,
        })
    }
}

#[async_trait]
impl WalletPregenerator for PrivyPregeneration {
    async fn pregenerate(&self, email: &str) -> Result<(), PregenerateError> {
        let response = self
            .http
            .post(PREGENERATE_URL)
            .basic_auth(&self.app_id, Some(&self.app_secret))
            .header("privy-app-id", &self.app_id)
            .json(&serde_json::json!({
                "linked_accounts": [{"address": email, "type": "email"}],
                "wallets": [{"chain_type": "ethereum"}],
            }))
            .send()
            .await
            .map_err(|error| {
                PregenerateError::Unavailable(format!("pregenerate wallet: {error}"))
            })?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(PregenerateError::Unavailable(format!(
                "pregenerate wallet answered {status}"
            )))
        }
    }
}

#[cfg(test)]
pub mod testing {
    //! A stand-in for Privy: remembers which mailboxes were asked for, and
    //! can be told to fail the way an outage would.

    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct FakePregenerator {
        pub asked: Mutex<Vec<String>>,
        pub outage: Mutex<bool>,
    }

    #[async_trait]
    impl WalletPregenerator for FakePregenerator {
        async fn pregenerate(&self, email: &str) -> Result<(), PregenerateError> {
            if *self.outage.lock().unwrap() {
                return Err(PregenerateError::Unavailable("outage".into()));
            }
            self.asked.lock().unwrap().push(email.to_owned());
            Ok(())
        }
    }
}
