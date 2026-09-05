//! PDF attachments in object storage (product plan §4.2, guide §Slice 2.4).
//!
//! Merchants upload straight to the bucket through a presigned PUT; the
//! gateway never proxies the bytes. Finalization then inspects the stored
//! object itself — its type, its size, the malware scanner's verdict tag, and
//! the PDF magic bytes — and hashes it, because nothing the client said about
//! the file is trusted. Objects are never moved: the key reserved at upload
//! is the key for life, and issuance only rewrites one tag.
//!
//! Two things keep the bytes an invoice committed to from changing under it.
//! The presigned PUT carries `If-None-Match: *`, so the key is write-once for
//! as long as the object exists. And finalization records the bucket version
//! it hashed; retagging and downloads pin that version, so even a PUT that
//! somehow lands afterwards is never what the invoice serves.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::B256;
use async_trait::async_trait;
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
#[cfg(test)]
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::types::{Tag, Tagging};
use chrono::{DateTime, Utc};
use gateway_core::PDF_MIME_TYPE;
use gateway_db::AccountId;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// Product plan §4.2: one PDF of at most 5 MiB.
pub const MAX_ATTACHMENT_BYTES: u64 = 5 * 1024 * 1024;
/// The verdict GuardDuty Malware Protection writes on a scanned object. Only
/// this value admits an upload (cross-slice decision 3); locally the e2e
/// suite stamps it by hand because nothing scans MinIO.
pub const SCAN_STATUS_TAG: &str = "GuardDutyMalwareScanStatus";
pub const CLEAN_SCAN: &str = "NO_THREATS_FOUND";
/// The lifecycle tag: `pending` uploads are expired by the bucket after a
/// bounded time, `attached` objects never are. Issuance flips it.
const UPLOAD_STATE_TAG: &str = "payday-upload";
const UPLOAD_PENDING: &str = "pending";
const UPLOAD_ATTACHED: &str = "attached";
const PDF_MAGIC: &[u8] = b"%PDF-";
/// How long a merchant has to PUT after reserving a slot.
const UPLOAD_TTL: Duration = Duration::from_secs(15 * 60);

/// What `HeadObject` reports about a stored object.
pub struct ObjectHead {
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    /// The current version on a versioned bucket; `None` where versioning is
    /// off (a local MinIO, for instance).
    pub version_id: Option<String>,
}

/// A request the client performs itself, with every signed header it must
/// repeat verbatim.
pub struct PresignedRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("object does not exist")]
    NotFound,
    #[error("object exceeds {0} bytes")]
    TooLarge(u64),
    #[error("object storage error: {0}")]
    Backend(String),
}

/// The handful of bucket operations the gateway needs, behind a trait so the
/// route tests run against memory instead of S3. Every read of an object
/// that has been finalized passes the pinned `version`; `None` means the
/// current version, which is only right before finalization.
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    async fn head(&self, key: &str, version: Option<&str>) -> Result<ObjectHead, StorageError>;
    async fn tags(
        &self,
        key: &str,
        version: Option<&str>,
    ) -> Result<BTreeMap<String, String>, StorageError>;
    /// The object's bytes, streamed and abandoned with [`StorageError::TooLarge`]
    /// as soon as more than `limit` bytes have arrived.
    #[cfg(test)]
    async fn get(
        &self,
        key: &str,
        version: Option<&str>,
        limit: u64,
    ) -> Result<Vec<u8>, StorageError>;
    /// Stream the object while checking its PDF prefix and computing SHA-256.
    async fn inspect_bytes(
        &self,
        key: &str,
        version: Option<&str>,
        limit: u64,
    ) -> Result<(u64, bool, [u8; 32]), StorageError>;
    async fn put_tags(
        &self,
        key: &str,
        version: Option<&str>,
        tags: &BTreeMap<String, String>,
    ) -> Result<(), StorageError>;
    /// Remove the current object under `key`. On a versioned bucket this
    /// leaves a delete marker; the lifecycle rule purges what is underneath.
    async fn delete(&self, key: &str) -> Result<(), StorageError>;
    async fn presign_put(
        &self,
        key: &str,
        content_type: &str,
        tagging: &str,
        ttl: Duration,
    ) -> Result<PresignedRequest, StorageError>;
    async fn presign_get(
        &self,
        key: &str,
        version: Option<&str>,
        filename: &str,
        ttl: Duration,
    ) -> Result<String, StorageError>;
}

/// The S3 bucket (or MinIO locally, through the endpoint override).
pub struct S3ObjectStorage {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3ObjectStorage {
    pub fn new(
        sdk_config: &aws_config::SdkConfig,
        bucket: String,
        endpoint: Option<&str>,
        force_path_style: bool,
    ) -> Self {
        let mut config =
            aws_sdk_s3::config::Builder::from(sdk_config).force_path_style(force_path_style);
        if let Some(endpoint) = endpoint {
            config = config.endpoint_url(endpoint);
        }
        Self {
            client: aws_sdk_s3::Client::from_conf(config.build()),
            bucket,
        }
    }
}

fn backend<E>(error: SdkError<E>) -> StorageError
where
    E: std::error::Error + Send + Sync + 'static,
{
    StorageError::Backend(DisplayErrorContext(&error).to_string())
}

/// S3 answers a key it does not hold with `NoSuchKey`, and a pinned version
/// the lifecycle rule has purged with `NoSuchVersion`; both mean the bytes
/// the caller wanted are gone.
fn is_missing<E: ProvideErrorMetadata>(error: &SdkError<E>) -> bool {
    matches!(
        error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code),
        Some("NoSuchKey" | "NoSuchVersion")
    )
}

fn missing_or_backend<E>(error: SdkError<E>) -> StorageError
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
{
    if is_missing(&error) {
        StorageError::NotFound
    } else {
        backend(error)
    }
}

#[async_trait]
impl ObjectStorage for S3ObjectStorage {
    async fn head(&self, key: &str, version: Option<&str>) -> Result<ObjectHead, StorageError> {
        let output = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .send()
            .await
            .map_err(|error| {
                if error
                    .as_service_error()
                    .is_some_and(HeadObjectError::is_not_found)
                {
                    StorageError::NotFound
                } else {
                    backend(error)
                }
            })?;
        Ok(ObjectHead {
            content_type: output.content_type().map(str::to_owned),
            content_length: output
                .content_length()
                .and_then(|length| u64::try_from(length).ok()),
            version_id: output.version_id().map(str::to_owned),
        })
    }

    async fn tags(
        &self,
        key: &str,
        version: Option<&str>,
    ) -> Result<BTreeMap<String, String>, StorageError> {
        let output = self
            .client
            .get_object_tagging()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .send()
            .await
            .map_err(missing_or_backend)?;
        Ok(output
            .tag_set()
            .iter()
            .map(|tag| (tag.key().to_owned(), tag.value().to_owned()))
            .collect())
    }

    #[cfg(test)]
    async fn get(
        &self,
        key: &str,
        version: Option<&str>,
        limit: u64,
    ) -> Result<Vec<u8>, StorageError> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .send()
            .await
            .map_err(|error| {
                if error
                    .as_service_error()
                    .is_some_and(GetObjectError::is_no_such_key)
                    || is_missing(&error)
                {
                    StorageError::NotFound
                } else {
                    backend(error)
                }
            })?;
        let mut body = output.body;
        let mut bytes = Vec::new();
        while let Some(chunk) = body
            .try_next()
            .await
            .map_err(|error| StorageError::Backend(error.to_string()))?
        {
            if (bytes.len() + chunk.len()) as u64 > limit {
                return Err(StorageError::TooLarge(limit));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn inspect_bytes(
        &self,
        key: &str,
        version: Option<&str>,
        limit: u64,
    ) -> Result<(u64, bool, [u8; 32]), StorageError> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .send()
            .await
            .map_err(missing_or_backend)?;
        let mut body = output.body;
        let mut length = 0u64;
        let mut prefix = Vec::with_capacity(PDF_MAGIC.len());
        let mut digest = Sha256::new();
        while let Some(chunk) = body
            .try_next()
            .await
            .map_err(|error| StorageError::Backend(error.to_string()))?
        {
            length = length.saturating_add(chunk.len() as u64);
            if length > limit {
                return Err(StorageError::TooLarge(limit));
            }
            if prefix.len() < PDF_MAGIC.len() {
                prefix.extend_from_slice(&chunk[..chunk.len().min(PDF_MAGIC.len() - prefix.len())]);
            }
            digest.update(&chunk);
        }
        Ok((length, prefix == PDF_MAGIC, digest.finalize().into()))
    }

    async fn put_tags(
        &self,
        key: &str,
        version: Option<&str>,
        tags: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        let tag_set = tags
            .iter()
            .map(|(name, value)| Tag::builder().key(name).value(value).build())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        let tagging = Tagging::builder()
            .set_tag_set(Some(tag_set))
            .build()
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        self.client
            .put_object_tagging()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .tagging(tagging)
            .send()
            .await
            .map_err(missing_or_backend)?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        // The task role holds s3:DeleteObject, not DeleteObjectVersion: this
        // is the current-version delete, which S3 answers even for a key it
        // no longer holds.
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn presign_put(
        &self,
        key: &str,
        content_type: &str,
        tagging: &str,
        ttl: Duration,
    ) -> Result<PresignedRequest, StorageError> {
        let presigning = PresigningConfig::expires_in(ttl)
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        // The content type, the lifecycle tag, and the write-once precondition
        // are part of the signature, so the client cannot upload anything but
        // a PDF under any other tag, and cannot replace an object that is
        // already there (S3 answers the second PUT with 412).
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .tagging(tagging)
            .if_none_match("*")
            .presigned(presigning)
            .await
            .map_err(backend)?;
        Ok(PresignedRequest {
            url: request.uri().to_owned(),
            headers: request
                .headers()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
                .collect(),
        })
    }

    async fn presign_get(
        &self,
        key: &str,
        version: Option<&str>,
        filename: &str,
        ttl: Duration,
    ) -> Result<String, StorageError> {
        let presigning = PresigningConfig::expires_in(ttl)
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .response_content_type(PDF_MIME_TYPE)
            .response_content_disposition(content_disposition(filename))
            .presigned(presigning)
            .await
            .map_err(backend)?;
        Ok(request.uri().to_owned())
    }
}

/// `attachment; filename="…"` with the name reduced to the characters that
/// are safe inside a quoted header value. The stored filename is verbatim;
/// only this rendering is sanitized.
pub fn content_disposition(filename: &str) -> String {
    let safe: String = filename
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' | ' ' => c,
            _ => '_',
        })
        .collect();
    let safe = safe.trim();
    let name = if safe.is_empty() {
        "attachment.pdf"
    } else {
        safe
    };
    format!("attachment; filename=\"{name}\"")
}

/// What the bucket must contain for an upload to become an attachment.
pub struct InspectedPdf {
    pub byte_length: u64,
    pub sha256: B256,
    /// The version the hash was computed over, to be pinned by every later
    /// read; `None` on an unversioned bucket.
    pub version_id: Option<String>,
}

pub struct PresignedUpload {
    pub attachment_id: Uuid,
    pub upload_url: String,
    pub headers: BTreeMap<String, String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum AttachmentError {
    #[error("the object has not been uploaded")]
    NotUploaded,
    #[error("the malware scan has not reported a verdict")]
    ScanPending,
    /// The reason is recorded on the attachment row; it is a short token or
    /// the scanner's verdict, never anything derived from the bytes.
    #[error("the upload was rejected: {0}")]
    Rejected(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Clone)]
pub struct AttachmentStore {
    storage: Arc<dyn ObjectStorage>,
    download_ttl: Duration,
}

impl AttachmentStore {
    pub fn new(storage: Arc<dyn ObjectStorage>, download_ttl: Duration) -> Self {
        Self {
            storage,
            download_ttl,
        }
    }

    /// `uploads/<account_id>/<attachment_id>.pdf`: the lifecycle rule and the
    /// task role's grants are both scoped to this prefix.
    pub fn object_key(account: AccountId, attachment_id: Uuid) -> String {
        format!("uploads/{}/{attachment_id}.pdf", account.0)
    }

    pub async fn presign_upload(
        &self,
        account: AccountId,
        attachment_id: Uuid,
    ) -> Result<PresignedUpload, AttachmentError> {
        let key = Self::object_key(account, attachment_id);
        let expires_at = Utc::now() + UPLOAD_TTL;
        let request = self
            .storage
            .presign_put(
                &key,
                PDF_MIME_TYPE,
                &format!("{UPLOAD_STATE_TAG}={UPLOAD_PENDING}"),
                UPLOAD_TTL,
            )
            .await?;
        Ok(PresignedUpload {
            attachment_id,
            upload_url: request.url,
            headers: request.headers,
            expires_at,
        })
    }

    /// The finalization order from the guide: head, type, size, tags, verdict,
    /// then the bytes themselves — size again, magic bytes, SHA-256. Every
    /// check reads the object; nothing the uploader claimed is consulted.
    /// The head names the version, and every later step reads that version,
    /// so the tags and the bytes belong to the object that was measured.
    pub async fn inspect_and_hash(
        &self,
        object_key: &str,
    ) -> Result<InspectedPdf, AttachmentError> {
        let head = match self.storage.head(object_key, None).await {
            Ok(head) => head,
            Err(StorageError::NotFound) => return Err(AttachmentError::NotUploaded),
            Err(error) => return Err(error.into()),
        };
        if head.content_type.as_deref().map(media_type) != Some(PDF_MIME_TYPE.to_owned()) {
            return Err(AttachmentError::Rejected("content_type_not_pdf".into()));
        }
        if !head
            .content_length
            .is_some_and(|length| (1..=MAX_ATTACHMENT_BYTES).contains(&length))
        {
            return Err(AttachmentError::Rejected("size_out_of_range".into()));
        }
        let version = head.version_id.as_deref();
        let tags = self.storage.tags(object_key, version).await?;
        match tags.get(SCAN_STATUS_TAG) {
            None => return Err(AttachmentError::ScanPending),
            Some(verdict) if verdict == CLEAN_SCAN => {}
            Some(verdict) => return Err(AttachmentError::Rejected(verdict.clone())),
        }
        let (byte_length, is_pdf, digest) = match self
            .storage
            .inspect_bytes(object_key, version, MAX_ATTACHMENT_BYTES)
            .await
        {
            Ok(inspected) => inspected,
            Err(StorageError::TooLarge(_)) => {
                return Err(AttachmentError::Rejected("size_out_of_range".into()));
            }
            Err(StorageError::NotFound) => return Err(AttachmentError::NotUploaded),
            Err(error) => return Err(error.into()),
        };
        if byte_length == 0 {
            return Err(AttachmentError::Rejected("size_out_of_range".into()));
        }
        if !is_pdf {
            return Err(AttachmentError::Rejected("not_a_pdf".into()));
        }
        Ok(InspectedPdf {
            byte_length,
            sha256: B256::from(digest),
            version_id: head.version_id,
        })
    }

    /// Whether the finalized object is still in the bucket. A ready upload
    /// that nobody issued with is expired by the lifecycle rule; the row
    /// only learns that when someone asks.
    pub async fn is_present(
        &self,
        object_key: &str,
        version: Option<&str>,
    ) -> Result<bool, AttachmentError> {
        match self.storage.head(object_key, version).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn presign_download(
        &self,
        object_key: &str,
        version: Option<&str>,
        filename: &str,
    ) -> Result<String, AttachmentError> {
        Ok(self
            .storage
            .presign_get(object_key, version, filename, self.download_ttl)
            .await?)
    }

    /// Remove a rejected upload's object. The bucket would expire it on its
    /// own in time, but there is no reason to keep an oversized, mistyped, or
    /// malware-flagged object around for a week.
    pub async fn delete(&self, object_key: &str) -> Result<(), AttachmentError> {
        Ok(self.storage.delete(object_key).await?)
    }

    /// Flip the lifecycle tag to `attached`, keeping every other tag (the
    /// scanner's verdict included) so the bucket's expiry rule stops matching
    /// the object. This is the only write issuance makes to the bucket.
    pub async fn mark_attached(
        &self,
        object_key: &str,
        version: Option<&str>,
    ) -> Result<(), AttachmentError> {
        self.set_upload_state(object_key, version, UPLOAD_ATTACHED)
            .await
    }

    /// Undo [`Self::mark_attached`] when the issuance it preceded did not
    /// commit, so the object is expired like any other unattached upload
    /// instead of lingering forever.
    pub async fn restore_pending(
        &self,
        object_key: &str,
        version: Option<&str>,
    ) -> Result<(), AttachmentError> {
        self.set_upload_state(object_key, version, UPLOAD_PENDING)
            .await
    }

    async fn set_upload_state(
        &self,
        object_key: &str,
        version: Option<&str>,
        state: &str,
    ) -> Result<(), AttachmentError> {
        let mut tags = self.storage.tags(object_key, version).await?;
        tags.insert(UPLOAD_STATE_TAG.into(), state.into());
        self.storage.put_tags(object_key, version, &tags).await?;
        Ok(())
    }
}

/// The media type without parameters, lowercased: `application/pdf` however
/// the uploader spelled it.
fn media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// An in-memory versioned bucket for tests: what a client PUT and what the
/// scanner tagged are set directly, and every PUT is a new version so the
/// tests can prove that reads stay pinned. Tags of a key that was never
/// uploaded read as empty and can be written, so database-only attachment
/// fixtures still issue.
#[cfg(test)]
pub mod memory {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryVersion {
        id: String,
        uploaded: bool,
        content_type: String,
        bytes: Vec<u8>,
        tags: BTreeMap<String, String>,
    }

    /// The versions of one key, oldest first; the last one is current.
    #[derive(Default)]
    struct MemoryObject {
        versions: Vec<MemoryVersion>,
    }

    impl MemoryObject {
        fn version(&self, version: Option<&str>) -> Option<&MemoryVersion> {
            match version {
                Some(id) => self.versions.iter().find(|candidate| candidate.id == id),
                None => self.versions.last(),
            }
        }

        fn version_mut(&mut self, version: Option<&str>) -> Option<&mut MemoryVersion> {
            match version {
                Some(id) => self
                    .versions
                    .iter_mut()
                    .find(|candidate| candidate.id == id),
                None => self.versions.last_mut(),
            }
        }
    }

    #[derive(Default)]
    struct MemoryBucket {
        objects: HashMap<String, MemoryObject>,
        next_version: u64,
    }

    impl MemoryBucket {
        fn new_version(&mut self, key: &str) -> &mut MemoryVersion {
            self.next_version += 1;
            let object = self.objects.entry(key.to_owned()).or_default();
            object.versions.push(MemoryVersion {
                id: format!("v{}", self.next_version),
                ..MemoryVersion::default()
            });
            object.versions.last_mut().expect("just pushed")
        }

        /// The current version, created unuploaded when the key is unknown.
        fn current_mut(&mut self, key: &str) -> &mut MemoryVersion {
            if self
                .objects
                .get(key)
                .is_some_and(|object| !object.versions.is_empty())
            {
                return self
                    .objects
                    .get_mut(key)
                    .and_then(|object| object.versions.last_mut())
                    .expect("checked above");
            }
            self.new_version(key)
        }
    }

    #[derive(Default)]
    pub struct MemoryObjectStorage {
        bucket: Mutex<MemoryBucket>,
    }

    impl MemoryObjectStorage {
        /// What a presigned PUT leaves behind: a new version holding the
        /// bytes, the declared type, and the lifecycle tag S3 takes from
        /// `x-amz-tagging`. Returns the version id.
        pub fn put(&self, key: &str, content_type: &str, bytes: &[u8]) -> String {
            let mut bucket = self.bucket.lock().unwrap();
            let version = bucket.new_version(key);
            version.uploaded = true;
            version.content_type = content_type.to_owned();
            version.bytes = bytes.to_vec();
            version
                .tags
                .insert(UPLOAD_STATE_TAG.into(), UPLOAD_PENDING.into());
            version.id.clone()
        }

        /// What the scanner does: tag the current version.
        pub fn tag(&self, key: &str, name: &str, value: &str) {
            self.bucket
                .lock()
                .unwrap()
                .current_mut(key)
                .tags
                .insert(name.to_owned(), value.to_owned());
        }

        /// The current version's tags.
        pub fn tags_of(&self, key: &str) -> BTreeMap<String, String> {
            self.tags_of_version(key, None)
        }

        pub fn tags_of_version(
            &self,
            key: &str,
            version: Option<&str>,
        ) -> BTreeMap<String, String> {
            self.bucket
                .lock()
                .unwrap()
                .objects
                .get(key)
                .and_then(|object| object.version(version))
                .map(|version| version.tags.clone())
                .unwrap_or_default()
        }

        /// What the lifecycle rule does to an upload nobody issued with.
        pub fn expire(&self, key: &str) {
            self.bucket.lock().unwrap().objects.remove(key);
        }

        /// Whether a client's upload is the current version of the key.
        pub fn exists(&self, key: &str) -> bool {
            self.bucket
                .lock()
                .unwrap()
                .objects
                .get(key)
                .and_then(|object| object.version(None))
                .is_some_and(|version| version.uploaded)
        }
    }

    #[async_trait]
    impl ObjectStorage for MemoryObjectStorage {
        async fn head(&self, key: &str, version: Option<&str>) -> Result<ObjectHead, StorageError> {
            let bucket = self.bucket.lock().unwrap();
            let object = bucket
                .objects
                .get(key)
                .and_then(|object| object.version(version))
                .filter(|object| object.uploaded)
                .ok_or(StorageError::NotFound)?;
            Ok(ObjectHead {
                content_type: Some(object.content_type.clone()),
                content_length: Some(object.bytes.len() as u64),
                version_id: Some(object.id.clone()),
            })
        }

        async fn tags(
            &self,
            key: &str,
            version: Option<&str>,
        ) -> Result<BTreeMap<String, String>, StorageError> {
            let bucket = self.bucket.lock().unwrap();
            let object = bucket.objects.get(key);
            match version {
                Some(_) => object
                    .and_then(|object| object.version(version))
                    .map(|version| version.tags.clone())
                    .ok_or(StorageError::NotFound),
                None => Ok(object
                    .and_then(|object| object.version(None))
                    .map(|version| version.tags.clone())
                    .unwrap_or_default()),
            }
        }

        #[cfg(test)]
        async fn get(
            &self,
            key: &str,
            version: Option<&str>,
            limit: u64,
        ) -> Result<Vec<u8>, StorageError> {
            let bucket = self.bucket.lock().unwrap();
            let object = bucket
                .objects
                .get(key)
                .and_then(|object| object.version(version))
                .filter(|object| object.uploaded)
                .ok_or(StorageError::NotFound)?;
            if object.bytes.len() as u64 > limit {
                return Err(StorageError::TooLarge(limit));
            }
            Ok(object.bytes.clone())
        }

        async fn inspect_bytes(
            &self,
            key: &str,
            version: Option<&str>,
            limit: u64,
        ) -> Result<(u64, bool, [u8; 32]), StorageError> {
            let bytes = self.get(key, version, limit).await?;
            let digest = Sha256::digest(&bytes).into();
            Ok((bytes.len() as u64, bytes.starts_with(PDF_MAGIC), digest))
        }

        async fn put_tags(
            &self,
            key: &str,
            version: Option<&str>,
            tags: &BTreeMap<String, String>,
        ) -> Result<(), StorageError> {
            let mut bucket = self.bucket.lock().unwrap();
            let target = match version {
                Some(_) => bucket
                    .objects
                    .get_mut(key)
                    .and_then(|object| object.version_mut(version))
                    .ok_or(StorageError::NotFound)?,
                None => bucket.current_mut(key),
            };
            target.tags = tags.clone();
            Ok(())
        }

        /// Forgets the key outright. A versioned bucket would keep the
        /// versions under a delete marker, but nothing reads them after a
        /// delete, so the simpler model is enough.
        async fn delete(&self, key: &str) -> Result<(), StorageError> {
            self.bucket.lock().unwrap().objects.remove(key);
            Ok(())
        }

        async fn presign_put(
            &self,
            key: &str,
            content_type: &str,
            tagging: &str,
            _ttl: Duration,
        ) -> Result<PresignedRequest, StorageError> {
            Ok(PresignedRequest {
                url: format!("memory://{key}?put"),
                headers: BTreeMap::from([
                    ("content-type".into(), content_type.into()),
                    ("if-none-match".into(), "*".into()),
                    ("x-amz-tagging".into(), tagging.into()),
                ]),
            })
        }

        async fn presign_get(
            &self,
            key: &str,
            version: Option<&str>,
            filename: &str,
            _ttl: Duration,
        ) -> Result<String, StorageError> {
            let mut url = format!("memory://{key}?download&filename={filename}");
            if let Some(version) = version {
                url.push_str(&format!("&versionId={version}"));
            }
            Ok(url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::memory::MemoryObjectStorage;
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF";

    fn store() -> (Arc<MemoryObjectStorage>, AttachmentStore) {
        let storage = Arc::new(MemoryObjectStorage::default());
        let store = AttachmentStore::new(storage.clone(), Duration::from_secs(300));
        (storage, store)
    }

    fn rejected(result: Result<InspectedPdf, AttachmentError>) -> String {
        match result {
            Err(AttachmentError::Rejected(reason)) => reason,
            Err(other) => panic!("expected a rejection, got {other}"),
            Ok(_) => panic!("expected a rejection, got a hash"),
        }
    }

    #[tokio::test]
    async fn finalization_checks_type_size_verdict_and_magic_bytes_in_order() {
        let (storage, store) = store();
        let key = AttachmentStore::object_key(AccountId(Uuid::from_u128(1)), Uuid::from_u128(2));
        assert_eq!(
            key,
            "uploads/00000000-0000-0000-0000-000000000001/00000000-0000-0000-0000-000000000002.pdf"
        );
        assert!(matches!(
            store.inspect_and_hash(&key).await,
            Err(AttachmentError::NotUploaded)
        ));

        // The scanner's verdict is only consulted once the object is a
        // plausible PDF by type and size.
        storage.put(&key, "text/plain", PDF);
        assert_eq!(
            rejected(store.inspect_and_hash(&key).await),
            "content_type_not_pdf"
        );
        storage.put(&key, "application/pdf; charset=binary", b"");
        assert_eq!(
            rejected(store.inspect_and_hash(&key).await),
            "size_out_of_range"
        );
        storage.put(
            &key,
            "application/pdf",
            &vec![b'%'; MAX_ATTACHMENT_BYTES as usize + 1],
        );
        assert_eq!(
            rejected(store.inspect_and_hash(&key).await),
            "size_out_of_range"
        );

        storage.put(&key, "Application/PDF", b"not a pdf at all");
        assert!(matches!(
            store.inspect_and_hash(&key).await,
            Err(AttachmentError::ScanPending)
        ));
        storage.tag(&key, SCAN_STATUS_TAG, "THREATS_FOUND");
        assert_eq!(
            rejected(store.inspect_and_hash(&key).await),
            "THREATS_FOUND"
        );
        storage.tag(&key, SCAN_STATUS_TAG, CLEAN_SCAN);
        assert_eq!(rejected(store.inspect_and_hash(&key).await), "not_a_pdf");

        let version = storage.put(&key, "application/pdf", PDF);
        storage.tag(&key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let inspected = store.inspect_and_hash(&key).await.unwrap();
        assert_eq!(inspected.byte_length, PDF.len() as u64);
        let expected: [u8; 32] = Sha256::digest(PDF).into();
        assert_eq!(inspected.sha256, B256::from(expected));
        assert_eq!(inspected.version_id.as_deref(), Some(version.as_str()));

        // A rejected object is removed; the store then reads nothing.
        store.delete(&key).await.unwrap();
        assert!(!storage.exists(&key));
        assert!(!store.is_present(&key, None).await.unwrap());
        assert!(matches!(
            store.inspect_and_hash(&key).await,
            Err(AttachmentError::NotUploaded)
        ));
    }

    #[tokio::test]
    async fn issuance_retags_the_object_and_keeps_the_scan_verdict() {
        let (storage, store) = store();
        let key = "uploads/a/b.pdf";
        let version = storage.put(key, "application/pdf", PDF);
        storage.tag(key, SCAN_STATUS_TAG, CLEAN_SCAN);
        assert_eq!(storage.tags_of(key)[UPLOAD_STATE_TAG], UPLOAD_PENDING);

        store.mark_attached(key, Some(&version)).await.unwrap();
        let tags = storage.tags_of(key);
        assert_eq!(tags[UPLOAD_STATE_TAG], UPLOAD_ATTACHED);
        assert_eq!(tags[SCAN_STATUS_TAG], CLEAN_SCAN);

        // Compensation after a failed issuance hands the object back to the
        // lifecycle rule.
        store.restore_pending(key, Some(&version)).await.unwrap();
        let tags = storage.tags_of(key);
        assert_eq!(tags[UPLOAD_STATE_TAG], UPLOAD_PENDING);
        assert_eq!(tags[SCAN_STATUS_TAG], CLEAN_SCAN);

        let upload = store
            .presign_upload(AccountId(Uuid::from_u128(1)), Uuid::from_u128(3))
            .await
            .unwrap();
        assert_eq!(upload.headers["content-type"], "application/pdf");
        assert_eq!(upload.headers["x-amz-tagging"], "payday-upload=pending");
        assert_eq!(
            upload.headers["if-none-match"], "*",
            "the PUT is write-once"
        );
        assert!(upload.expires_at > Utc::now());
    }

    #[tokio::test]
    async fn reads_after_finalization_stay_pinned_to_the_hashed_version() {
        let (storage, store) = store();
        let key = "uploads/a/pinned.pdf";
        storage.put(key, "application/pdf", PDF);
        storage.tag(key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let inspected = store.inspect_and_hash(key).await.unwrap();
        let pinned = inspected.version_id.clone().unwrap();

        // Someone gets a second PUT past the precondition: a new current
        // version with different bytes and a fresh pending tag.
        let replaced = storage.put(key, "application/pdf", b"%PDF-1.4 something else");
        assert_ne!(replaced, pinned);

        let bytes = storage
            .get(key, Some(&pinned), MAX_ATTACHMENT_BYTES)
            .await
            .unwrap();
        assert_eq!(bytes, PDF, "the hashed bytes are what is read");
        let url = store
            .presign_download(key, Some(&pinned), "contract.pdf")
            .await
            .unwrap();
        assert!(
            url.ends_with(&format!("&versionId={pinned}")),
            "the download link names the hashed version: {url}"
        );

        store.mark_attached(key, Some(&pinned)).await.unwrap();
        assert_eq!(
            storage.tags_of_version(key, Some(&pinned))[UPLOAD_STATE_TAG],
            UPLOAD_ATTACHED
        );
        assert_eq!(
            storage.tags_of_version(key, Some(&replaced))[UPLOAD_STATE_TAG],
            UPLOAD_PENDING,
            "the replacement stays subject to lifecycle expiry"
        );
        assert!(store.is_present(key, Some(&pinned)).await.unwrap());
        assert!(!store.is_present(key, Some("v999")).await.unwrap());
        assert!(matches!(
            store.mark_attached(key, Some("v999")).await,
            Err(AttachmentError::Storage(StorageError::NotFound))
        ));
    }

    /// Against a real bucket (MinIO locally): proves the presigned PUT signs
    /// the headers the client must repeat, that a second PUT to the key is
    /// refused, that MinIO stores the lifecycle tag from `x-amz-tagging`, and
    /// that every finalization step reads what the client actually wrote. On
    /// a versioned bucket it also proves that reads stay pinned to the
    /// hashed version after an unconditional overwrite. Needs
    /// PAYDAY_ATTACHMENT_BUCKET, PAYDAY_ATTACHMENT_S3_ENDPOINT, and AWS_*
    /// credentials in the environment, so it is opt-in:
    /// `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a reachable S3-compatible bucket"]
    async fn s3_storage_round_trips_against_the_configured_bucket() {
        let bucket = std::env::var("PAYDAY_ATTACHMENT_BUCKET").unwrap();
        let endpoint = std::env::var("PAYDAY_ATTACHMENT_S3_ENDPOINT").ok();
        let sdk = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let storage: Arc<dyn ObjectStorage> = Arc::new(S3ObjectStorage::new(
            &sdk,
            bucket.clone(),
            endpoint.as_deref(),
            true,
        ));
        let store = AttachmentStore::new(storage.clone(), Duration::from_secs(60));
        let account = AccountId(Uuid::now_v7());
        let attachment_id = Uuid::now_v7();
        let key = AttachmentStore::object_key(account, attachment_id);
        assert!(matches!(
            store.inspect_and_hash(&key).await,
            Err(AttachmentError::NotUploaded)
        ));
        assert!(matches!(
            storage.tags(&key, None).await,
            Err(StorageError::NotFound)
        ));

        // The client's PUT, with exactly the signed headers.
        let upload = store.presign_upload(account, attachment_id).await.unwrap();
        assert_eq!(upload.headers["content-type"], "application/pdf");
        assert_eq!(upload.headers["x-amz-tagging"], "payday-upload=pending");
        assert_eq!(upload.headers["if-none-match"], "*");
        let http = reqwest::Client::new();
        let put = |bytes: &'static [u8]| {
            let mut put = http.put(&upload.upload_url).body(bytes);
            for (name, value) in &upload.headers {
                put = put.header(name, value);
            }
            put.send()
        };
        assert!(put(PDF).await.unwrap().status().is_success());
        // The same presigned URL cannot replace what is there.
        assert_eq!(
            put(b"%PDF-1.4 replaced").await.unwrap().status(),
            reqwest::StatusCode::PRECONDITION_FAILED,
            "the key is write-once"
        );

        let head = storage.head(&key, None).await.unwrap();
        assert_eq!(head.content_type.as_deref(), Some("application/pdf"));
        assert_eq!(head.content_length, Some(PDF.len() as u64));
        assert_eq!(
            storage.tags(&key, None).await.unwrap()[UPLOAD_STATE_TAG],
            UPLOAD_PENDING
        );
        assert!(matches!(
            store.inspect_and_hash(&key).await,
            Err(AttachmentError::ScanPending)
        ));

        // The scanner's verdict, then finalization and issuance.
        let mut tags = storage.tags(&key, None).await.unwrap();
        tags.insert(SCAN_STATUS_TAG.into(), CLEAN_SCAN.into());
        storage.put_tags(&key, None, &tags).await.unwrap();
        let inspected = store.inspect_and_hash(&key).await.unwrap();
        let expected: [u8; 32] = Sha256::digest(PDF).into();
        assert_eq!(inspected.sha256, B256::from(expected));
        assert_eq!(inspected.byte_length, PDF.len() as u64);
        let pinned = inspected.version_id.clone();
        assert_eq!(pinned, head.version_id);

        if let Some(pinned) = pinned.as_deref() {
            // An unconditional overwrite by whoever holds bucket credentials
            // becomes the current version; the pinned reads ignore it.
            client_put(
                &sdk,
                endpoint.as_deref(),
                &bucket,
                &key,
                "application/pdf",
                b"%PDF-1.4 replaced",
            )
            .await;
            let current = storage.head(&key, None).await.unwrap();
            assert_ne!(current.version_id.as_deref(), Some(pinned));
            assert_eq!(
                storage
                    .get(&key, Some(pinned), MAX_ATTACHMENT_BYTES)
                    .await
                    .unwrap(),
                PDF
            );
            store.mark_attached(&key, Some(pinned)).await.unwrap();
            assert_eq!(
                storage.tags(&key, Some(pinned)).await.unwrap()[UPLOAD_STATE_TAG],
                UPLOAD_ATTACHED
            );
            assert_eq!(
                storage.tags(&key, None).await.unwrap()[UPLOAD_STATE_TAG],
                UPLOAD_PENDING,
                "the overwrite is still expired by the lifecycle rule"
            );
        } else {
            store.mark_attached(&key, None).await.unwrap();
        }
        let tags = storage.tags(&key, pinned.as_deref()).await.unwrap();
        assert_eq!(tags[UPLOAD_STATE_TAG], UPLOAD_ATTACHED);
        assert_eq!(tags[SCAN_STATUS_TAG], CLEAN_SCAN);

        // The signed download link serves the hashed bytes as a PDF attachment.
        let url = store
            .presign_download(&key, pinned.as_deref(), "contract.pdf")
            .await
            .unwrap();
        let response = http.get(&url).send().await.unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"contract.pdf\""
        );
        assert_eq!(response.headers()["content-type"], "application/pdf");
        assert_eq!(response.bytes().await.unwrap().as_ref(), PDF);
        assert!(matches!(
            storage.get(&key, pinned.as_deref(), 4).await,
            Err(StorageError::TooLarge(4))
        ));

        // A rejected upload's object is deleted, and deleting it again is
        // not an error.
        let rejected = AttachmentStore::object_key(account, Uuid::now_v7());
        client_put(
            &sdk,
            endpoint.as_deref(),
            &bucket,
            &rejected,
            "text/plain",
            b"not a pdf",
        )
        .await;
        assert!(store.is_present(&rejected, None).await.unwrap());
        store.delete(&rejected).await.unwrap();
        assert!(!store.is_present(&rejected, None).await.unwrap());
        store.delete(&rejected).await.unwrap();
    }

    /// A PUT straight from bucket credentials, with no precondition: what an
    /// operator or a compromised role could do, and what the pinned reads
    /// must ignore.
    async fn client_put(
        sdk: &aws_config::SdkConfig,
        endpoint: Option<&str>,
        bucket: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) {
        let mut config = aws_sdk_s3::config::Builder::from(sdk).force_path_style(true);
        config.set_endpoint_url(endpoint.map(str::to_owned));
        aws_sdk_s3::Client::from_conf(config.build())
            .put_object()
            .bucket(bucket)
            .key(key)
            .content_type(content_type)
            .tagging("payday-upload=pending")
            .body(bytes.to_vec().into())
            .send()
            .await
            .unwrap();
    }

    #[test]
    fn download_disposition_keeps_only_header_safe_characters() {
        assert_eq!(
            content_disposition("March request.pdf"),
            "attachment; filename=\"March request.pdf\""
        );
        assert_eq!(
            content_disposition("a\"b\\c\r\n;é.pdf"),
            "attachment; filename=\"a_b_c____.pdf\""
        );
        assert_eq!(
            content_disposition("   "),
            "attachment; filename=\"attachment.pdf\""
        );
    }
}
