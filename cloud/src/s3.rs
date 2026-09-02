//! S3-compatible object storage for Garage and conforming endpoints.
//!
//! The AWS Rust SDK owns `SigV4` signing and presigning. This adapter supplies
//! the endpoint, credentials, and forced path-style addressing required by
//! Garage; no other workspace crate depends on an S3 provider API.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use crate::{ObjectListing, StorageError, StorageService, StoredObject};

/// Environment-derived settings for an S3-compatible storage lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3StorageConfig {
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

impl S3StorageConfig {
    pub fn from_env() -> Result<Self, StorageError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, StorageError> {
        Self::from_lookup_with_bucket(get, "NAVIGATOR_STORAGE", |get| {
            get("NAVIGATOR_DOCUMENTS_BUCKET")
                .filter(|value| !value.is_empty())
                .or_else(|| get("NAVIGATOR_STORAGE_BUCKET").filter(|value| !value.is_empty()))
                .ok_or(StorageError::MissingEnv(
                    "NAVIGATOR_DOCUMENTS_BUCKET or NAVIGATOR_STORAGE_BUCKET",
                ))
        })
    }

    pub fn exports_from_env() -> Result<Self, StorageError> {
        Self::exports_from_lookup(|key| std::env::var(key).ok())
    }

    pub fn surreal_archives_from_env() -> Result<Self, StorageError> {
        Self::surreal_archives_from_lookup(|key| std::env::var(key).ok())
    }

    pub fn exports_from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, StorageError> {
        Self::from_lookup_with_bucket(get, "NAVIGATOR_EXPORTS", |get| {
            get("NAVIGATOR_STORAGE_BUCKET")
                .filter(|value| !value.is_empty())
                .ok_or(StorageError::MissingEnv("NAVIGATOR_STORAGE_BUCKET"))
        })
    }

    pub fn surreal_archives_from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, StorageError> {
        Self::from_lookup_with_bucket(get, "NAVIGATOR_SURREAL_ARCHIVES", |get| {
            get("NAVIGATOR_SURREAL_ARCHIVES_BUCKET")
                .filter(|value| !value.is_empty())
                .ok_or(StorageError::MissingEnv(
                    "NAVIGATOR_SURREAL_ARCHIVES_BUCKET",
                ))
        })
    }

    pub fn assets_from_env() -> Result<Self, StorageError> {
        Self::assets_from_lookup(|key| std::env::var(key).ok())
    }

    pub fn lfs_from_env() -> Result<Self, StorageError> {
        Self::lfs_from_lookup(|key| std::env::var(key).ok())
    }

    pub fn lfs_from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, StorageError> {
        Self::from_lookup_with_bucket(get, "NAVIGATOR_LFS", |get| {
            get("NAVIGATOR_LFS_BUCKET")
                .filter(|value| !value.is_empty())
                .ok_or(StorageError::MissingEnv("NAVIGATOR_LFS_BUCKET"))
        })
    }

    pub fn assets_from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, StorageError> {
        Self::from_lookup_with_bucket(get, "NAVIGATOR_ASSETS", |get| {
            get("NAVIGATOR_ASSETS_BUCKET")
                .filter(|value| !value.is_empty())
                .or_else(|| get("NAVIGATOR_STORAGE_BUCKET").filter(|value| !value.is_empty()))
                .ok_or(StorageError::MissingEnv(
                    "NAVIGATOR_ASSETS_BUCKET or NAVIGATOR_STORAGE_BUCKET",
                ))
        })
    }

    pub fn applications_from_env() -> Result<Self, StorageError> {
        Self::applications_from_lookup(|key| std::env::var(key).ok())
    }

    pub fn applications_from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, StorageError> {
        Self::from_lookup_with_bucket(get, "NAVIGATOR_APPLICATIONS", |get| {
            get("NAVIGATOR_APPLICATIONS_BUCKET")
                .filter(|value| !value.is_empty())
                .or_else(|| get("NAVIGATOR_STORAGE_BUCKET").filter(|value| !value.is_empty()))
                .ok_or(StorageError::MissingEnv(
                    "NAVIGATOR_APPLICATIONS_BUCKET or NAVIGATOR_STORAGE_BUCKET",
                ))
        })
    }

    fn from_lookup_with_bucket<F, B>(
        get: F,
        lane: &str,
        bucket_from: B,
    ) -> Result<Self, StorageError>
    where
        F: Fn(&str) -> Option<String>,
        B: FnOnce(&F) -> Result<String, StorageError>,
    {
        let bucket = bucket_from(&get)?;
        let endpoint = required(&get, "NAVIGATOR_STORAGE_ENDPOINT")?;
        let access_name = format!("{lane}_ACCESS_KEY");
        let secret_name = format!("{lane}_SECRET_KEY");
        let access_key = get(&access_name)
            .filter(|value| !value.is_empty())
            .or_else(|| get("NAVIGATOR_STORAGE_ACCESS_KEY").filter(|value| !value.is_empty()))
            .ok_or(StorageError::MissingEnv("NAVIGATOR_STORAGE_ACCESS_KEY"))?;
        let secret_key = get(&secret_name)
            .filter(|value| !value.is_empty())
            .or_else(|| get("NAVIGATOR_STORAGE_SECRET_KEY").filter(|value| !value.is_empty()))
            .ok_or(StorageError::MissingEnv("NAVIGATOR_STORAGE_SECRET_KEY"))?;
        Ok(Self {
            bucket,
            endpoint,
            region: get("NAVIGATOR_STORAGE_REGION").unwrap_or_else(|| "garage".into()),
            access_key,
            secret_key,
            session_token: get("NAVIGATOR_STORAGE_SESSION_TOKEN").filter(|value| !value.is_empty()),
        })
    }
}

fn required<F: Fn(&str) -> Option<String>>(
    get: &F,
    key: &'static str,
) -> Result<String, StorageError> {
    get(key)
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::MissingEnv(key))
}

/// S3-compatible backend with forced path-style requests.
#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    bucket: Arc<str>,
}

impl S3Storage {
    pub fn new(config: S3StorageConfig) -> Result<Self, StorageError> {
        let credentials = Credentials::new(
            config.access_key,
            config.secret_key,
            config.session_token,
            None,
            "navigator-s3-env",
        );
        let sdk_config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region))
            .credentials_provider(credentials)
            .endpoint_url(config.endpoint)
            .force_path_style(true)
            .build();
        Ok(Self {
            client: Client::from_conf(sdk_config),
            bucket: Arc::from(config.bucket),
        })
    }

    fn failure(key: &str, error: impl std::fmt::Display) -> StorageError {
        let message = error.to_string();
        if message.contains("NotFound") || message.contains("NoSuchKey") || message.contains("404")
        {
            StorageError::NotFound(key.into())
        } else {
            StorageError::S3 {
                key: key.into(),
                message,
            }
        }
    }

    fn sdk_failure<E>(key: &str, error: &SdkError<E>) -> StorageError
    where
        E: ProvideErrorMetadata + std::fmt::Debug,
    {
        let code = error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code);
        if matches!(code, Some("NotFound" | "NoSuchKey"))
            || error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 404)
        {
            StorageError::NotFound(key.into())
        } else {
            StorageError::S3 {
                key: key.into(),
                message: code.unwrap_or("service error").into(),
            }
        }
    }
}

#[async_trait]
impl StorageService for S3Storage {
    async fn put(&self, key: &str, bytes: &[u8], content_type: &str) -> Result<(), StorageError> {
        self.client
            .put_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .body(ByteStream::from(bytes.to_vec()))
            .content_type(content_type)
            .send()
            .await
            .map_err(|error| Self::sdk_failure(key, &error))?;
        Ok(())
    }

    async fn put_cached(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), StorageError> {
        self.client
            .put_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .body(ByteStream::from(bytes.to_vec()))
            .content_type(content_type)
            .cache_control(cache_control)
            .send()
            .await
            .map_err(|error| Self::sdk_failure(key, &error))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        let object = self
            .client
            .get_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .send()
            .await
            .map_err(|error| Self::sdk_failure(key, &error))?;
        let bytes = object
            .body
            .collect()
            .await
            .map_err(|error| Self::failure(key, error))?
            .into_bytes()
            .to_vec();
        Ok(StoredObject {
            key: key.into(),
            bytes,
            content_type: object
                .content_type
                .unwrap_or_else(|| "application/octet-stream".into()),
        })
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.client
            .delete_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .send()
            .await
            .map_err(|error| Self::sdk_failure(key, &error))?;
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectListing>, StorageError> {
        let mut continuation_token = None;
        let mut listed = Vec::new();
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(self.bucket.as_ref())
                .prefix(prefix)
                .set_continuation_token(continuation_token)
                .send()
                .await
                .map_err(|error| Self::sdk_failure(prefix, &error))?;
            listed.extend(page.contents().iter().filter_map(|object| {
                Some(ObjectListing {
                    key: object.key()?.into(),
                    size_bytes: u64::try_from(object.size()?).ok()?,
                })
            }));
            continuation_token = page.next_continuation_token().map(str::to_owned);
            if continuation_token.is_none() {
                break;
            }
        }
        listed.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(listed)
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        match self
            .client
            .head_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(error) => match Self::sdk_failure(key, &error) {
                StorageError::NotFound(_) => Ok(false),
                other => Err(other),
            },
        }
    }

    async fn signed_url(&self, key: &str, expires_in: Duration) -> Result<String, StorageError> {
        let config =
            PresigningConfig::expires_in(expires_in).map_err(|error| Self::failure(key, error))?;
        let request = self
            .client
            .get_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .presigned(config)
            .await
            .map_err(|error| Self::failure(key, error))?;
        Ok(request.uri().to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{S3Storage, S3StorageConfig};
    use crate::{StorageError, StorageService};

    fn config(endpoint: String) -> S3StorageConfig {
        S3StorageConfig {
            bucket: "documents".into(),
            endpoint,
            region: "garage".into(),
            access_key: "access".into(),
            secret_key: "secret".into(),
            session_token: None,
        }
    }

    #[test]
    fn config_reads_sigv4_values_and_documents_lane() {
        let values = HashMap::from([
            ("NAVIGATOR_DOCUMENTS_BUCKET", "documents"),
            ("NAVIGATOR_STORAGE_BUCKET", "fallback"),
            ("NAVIGATOR_STORAGE_ENDPOINT", "http://garage:3900"),
            ("NAVIGATOR_STORAGE_ACCESS_KEY", "access"),
            ("NAVIGATOR_STORAGE_SECRET_KEY", "secret"),
        ]);
        let config =
            S3StorageConfig::from_lookup(|key| values.get(key).map(ToString::to_string)).unwrap();
        assert_eq!(config.bucket, "documents");
        assert_eq!(config.region, "garage");
    }

    #[test]
    fn config_requires_endpoint_and_credentials() {
        let error = S3StorageConfig::from_lookup(|_| None).unwrap_err();
        assert!(matches!(error, StorageError::MissingEnv(_)));
    }

    #[test]
    fn lfs_lane_requires_its_own_bucket_and_credentials() {
        let values = HashMap::from([
            ("NAVIGATOR_LFS_BUCKET", "lfs"),
            ("NAVIGATOR_STORAGE_ENDPOINT", "http://garage:3900"),
            ("NAVIGATOR_LFS_ACCESS_KEY", "lfs-access"),
            ("NAVIGATOR_LFS_SECRET_KEY", "lfs-secret"),
        ]);
        let config =
            S3StorageConfig::lfs_from_lookup(|key| values.get(key).map(ToString::to_string))
                .unwrap();
        assert_eq!(config.bucket, "lfs");
        assert_eq!(config.access_key, "lfs-access");
    }

    #[test]
    fn assets_and_exports_lanes_select_their_scoped_values() {
        let values = HashMap::from([
            ("NAVIGATOR_ASSETS_BUCKET", "assets"),
            ("NAVIGATOR_STORAGE_BUCKET", "exports"),
            ("NAVIGATOR_STORAGE_ENDPOINT", "http://garage:3900"),
            ("NAVIGATOR_STORAGE_REGION", "us-test-1"),
            ("NAVIGATOR_ASSETS_ACCESS_KEY", "assets-access"),
            ("NAVIGATOR_ASSETS_SECRET_KEY", "assets-secret"),
            ("NAVIGATOR_EXPORTS_ACCESS_KEY", "exports-access"),
            ("NAVIGATOR_EXPORTS_SECRET_KEY", "exports-secret"),
            ("NAVIGATOR_STORAGE_SESSION_TOKEN", "session"),
        ]);
        let assets =
            S3StorageConfig::assets_from_lookup(|key| values.get(key).map(ToString::to_string))
                .unwrap();
        assert_eq!(assets.bucket, "assets");
        assert_eq!(assets.region, "us-test-1");
        assert_eq!(assets.session_token.as_deref(), Some("session"));

        let exports =
            S3StorageConfig::exports_from_lookup(|key| values.get(key).map(ToString::to_string))
                .unwrap();
        assert_eq!(exports.bucket, "exports");
        assert_eq!(exports.access_key, "exports-access");
    }

    #[test]
    fn surreal_archive_lane_selects_its_bucket_and_credentials() {
        let values = HashMap::from([
            ("NAVIGATOR_SURREAL_ARCHIVES_BUCKET", "neon-law-archives"),
            ("NAVIGATOR_STORAGE_BUCKET", "exports"),
            ("NAVIGATOR_STORAGE_ENDPOINT", "http://garage:3900"),
            ("NAVIGATOR_SURREAL_ARCHIVES_ACCESS_KEY", "archive-access"),
            ("NAVIGATOR_SURREAL_ARCHIVES_SECRET_KEY", "archive-secret"),
        ]);
        let config = S3StorageConfig::surreal_archives_from_lookup(|key| {
            values.get(key).map(ToString::to_string)
        })
        .unwrap();
        assert_eq!(config.bucket, "neon-law-archives");
        assert_eq!(config.access_key, "archive-access");
    }

    #[test]
    fn surreal_archive_lane_requires_its_dedicated_bucket() {
        let values = HashMap::from([
            ("NAVIGATOR_DOCUMENTS_BUCKET", "documents"),
            ("NAVIGATOR_STORAGE_BUCKET", "exports"),
            ("NAVIGATOR_STORAGE_ENDPOINT", "http://garage:3900"),
            ("NAVIGATOR_STORAGE_ACCESS_KEY", "access"),
            ("NAVIGATOR_STORAGE_SECRET_KEY", "secret"),
        ]);
        let error = S3StorageConfig::surreal_archives_from_lookup(|key| {
            values.get(key).map(ToString::to_string)
        })
        .unwrap_err();
        assert!(matches!(
            error,
            StorageError::MissingEnv("NAVIGATOR_SURREAL_ARCHIVES_BUCKET")
        ));
    }

    #[tokio::test]
    async fn object_operations_use_path_style_s3_requests() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/documents/folder/item.txt"))
            .respond_with(ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/documents/folder/item.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_bytes(b"stored"),
            )
            .mount(&server)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/documents/folder/item.txt"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/documents/folder/item.txt"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let storage = S3Storage::new(config(server.uri())).unwrap();
        storage
            .put("folder/item.txt", b"stored", "text/plain")
            .await
            .unwrap();
        storage
            .put_cached(
                "folder/item.txt",
                b"stored",
                "text/plain",
                "private, max-age=60",
            )
            .await
            .unwrap();
        let object = storage.get("folder/item.txt").await.unwrap();
        assert_eq!(object.bytes, b"stored");
        assert_eq!(object.content_type, "text/plain");
        assert!(storage.exists("folder/item.txt").await.unwrap());
        storage.delete("folder/item.txt").await.unwrap();

        let signed = storage
            .signed_url("folder/item.txt", Duration::from_mins(1))
            .await
            .unwrap();
        assert!(signed.starts_with(&server.uri()));
        assert!(signed.contains("X-Amz-Signature="));
    }

    #[tokio::test]
    async fn list_maps_and_sorts_s3_objects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("^/documents/?$"))
            .and(query_param("list-type", "2"))
            .and(query_param("prefix", "folder/"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                   <Name>documents</Name><Prefix>folder/</Prefix><IsTruncated>false</IsTruncated>\
                   <Contents><Key>folder/z.txt</Key><Size>9</Size></Contents>\
                   <Contents><Key>folder/a.txt</Key><Size>3</Size></Contents>\
                 </ListBucketResult>",
                "application/xml",
            ))
            .mount(&server)
            .await;

        let storage = S3Storage::new(config(server.uri())).unwrap();
        let listed = storage.list("folder/").await.unwrap();
        assert_eq!(listed[0].key, "folder/a.txt");
        assert_eq!(listed[0].size_bytes, 3);
        assert_eq!(listed[1].key, "folder/z.txt");
    }

    #[tokio::test]
    async fn missing_and_service_errors_are_distinct() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/documents/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_raw(
                "<Error><Code>NoSuchKey</Code><Message>missing</Message></Error>",
                "application/xml",
            ))
            .mount(&server)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/documents/broken"))
            .respond_with(ResponseTemplate::new(500).set_body_raw(
                "<Error><Code>InternalError</Code><Message>broken</Message></Error>",
                "application/xml",
            ))
            .mount(&server)
            .await;

        let storage = S3Storage::new(config(server.uri())).unwrap();
        assert!(!storage.exists("missing").await.unwrap());
        assert!(matches!(
            storage.exists("broken").await,
            Err(StorageError::S3 { .. })
        ));
    }

    #[tokio::test]
    async fn operation_failures_map_to_storage_errors() {
        let server = MockServer::start().await;
        for verb in ["PUT", "GET", "DELETE"] {
            Mock::given(method(verb))
                .and(path("/documents/broken"))
                .respond_with(ResponseTemplate::new(500).set_body_raw(
                    "<Error><Code>InternalError</Code><Message>broken</Message></Error>",
                    "application/xml",
                ))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path_regex("^/documents/?$"))
            .respond_with(ResponseTemplate::new(500).set_body_raw(
                "<Error><Code>InternalError</Code><Message>broken</Message></Error>",
                "application/xml",
            ))
            .mount(&server)
            .await;

        let storage = S3Storage::new(config(server.uri())).unwrap();
        assert!(matches!(
            storage.put("broken", b"x", "text/plain").await,
            Err(StorageError::S3 { .. })
        ));
        assert!(matches!(
            storage
                .put_cached("broken", b"x", "text/plain", "no-cache")
                .await,
            Err(StorageError::S3 { .. })
        ));
        assert!(matches!(
            storage.get("broken").await,
            Err(StorageError::S3 { .. })
        ));
        assert!(matches!(
            storage.delete("broken").await,
            Err(StorageError::S3 { .. })
        ));
        assert!(matches!(
            storage.list("broken").await,
            Err(StorageError::S3 { .. })
        ));
        assert!(matches!(
            storage
                .signed_url("broken", Duration::from_hours(192))
                .await,
            Err(StorageError::S3 { .. })
        ));
    }

    #[tokio::test]
    async fn get_defaults_content_type_when_the_response_omits_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/documents/no-type"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"raw"))
            .mount(&server)
            .await;

        let storage = S3Storage::new(config(server.uri())).unwrap();
        let object = storage.get("no-type").await.unwrap();
        assert_eq!(object.bytes, b"raw");
        assert_eq!(object.content_type, "application/octet-stream");
    }

    #[test]
    fn streaming_failures_split_not_found_from_service_errors() {
        for message in ["NotFound", "NoSuchKey", "status 404 from upstream"] {
            assert!(matches!(
                S3Storage::failure("key", message),
                StorageError::NotFound(_)
            ));
        }
        assert!(matches!(
            S3Storage::failure("key", "connection reset"),
            StorageError::S3 { .. }
        ));
    }
}
