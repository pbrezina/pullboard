//! Read-only forge transport, conditional requests, bounded pagination and backoff.
use crate::{
    config::{ForgeConfig, JiraConfig, Upstream, http_url},
    error::{Error, Result},
    model::Kind,
    request_budget::{Bucket, RequestBudget},
};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Conditional {
    etag: HeaderValue,
    body: Bytes,
    headers: HeaderMap,
}
pub struct ReadTransport {
    client: reqwest::Client,
    kind: Kind,
    base: url::Url,
    limits: Upstream,
    cache: Mutex<BTreeMap<String, Conditional>>,
    budget: Arc<RequestBudget>,
}
type PageKey<'a, T> = &'a (dyn Fn(&T) -> Result<String> + Send + Sync);

pub fn http_client(limits: &Upstream) -> anyhow::Result<reqwest::Client> {
    http_client_builder(limits)
        .build()
        .map_err(|_| anyhow::anyhow!("HTTP client initialization failed"))
}

fn http_client_builder(limits: &Upstream) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent("PullBoard/0.1")
        .timeout(Duration::from_secs(limits.timeout_seconds))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        // Retries must be explicit, bounded and reacquire the origin budget.
        .retry(reqwest::retry::never())
}

impl ReadTransport {
    pub async fn new(config: &ForgeConfig, limits: &Upstream) -> anyhow::Result<Arc<Self>> {
        let secret = config.token.resolve().await?;
        let mut base = url::Url::parse(&format!("{}/", config.base_url.trim_end_matches('/')))
            .map_err(|_| anyhow::anyhow!("invalid forge base URL"))?;
        // Adapter API suffixes do not change the origin used by this budget.
        let budget = RequestBudget::shared(base.as_str(), limits)?;
        let mut headers = HeaderMap::new();
        let (auth_header, auth_value) = match config.kind {
            Kind::Github => {
                // Preserve Octocrab 0.54's defaults, including its API version.
                // No previews were configured, so it supplied no Accept header.
                headers.insert(
                    http::header::USER_AGENT,
                    HeaderValue::from_static("octocrab"),
                );
                headers.insert(
                    "x-github-api-version",
                    HeaderValue::from_static("2022-11-28"),
                );
                (http::header::AUTHORIZATION, format!("Bearer {}", *secret))
            }
            Kind::Gitlab => {
                base = base
                    .join("api/v4/")
                    .map_err(|_| anyhow::anyhow!("invalid GitLab API base URL"))?;
                (
                    http::header::HeaderName::from_static("private-token"),
                    secret.to_string(),
                )
            }
            Kind::Codeberg | Kind::Forgejo => {
                base = base
                    .join("api/v1/")
                    .map_err(|_| anyhow::anyhow!("invalid Forgejo API base URL"))?;
                (http::header::AUTHORIZATION, format!("token {}", *secret))
            }
            Kind::Jira => anyhow::bail!("Jira is not a forge"),
        };
        let mut value = HeaderValue::from_str(&auth_value)
            .map_err(|_| anyhow::anyhow!("invalid forge token"))?;
        value.set_sensitive(true);
        headers.insert(auth_header, value);
        // Construction never probes the network. Credentials and conditional
        // responses belong only to this configured instance, not a global client.
        let client = http_client_builder(limits)
            .default_headers(headers)
            // Only the explicit bounded REST loop may retry; every attempt
            // must reacquire the shared budget and recheck published backoff.
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| anyhow::anyhow!("forge client initialization failed"))?;
        Ok(Arc::new(Self {
            client,
            kind: config.kind,
            base,
            limits: limits.clone(),
            cache: Mutex::new(BTreeMap::new()),
            budget,
        }))
    }
    pub fn page_size(&self) -> u32 {
        self.limits.page_size
    }
    /// Validated YAML bounds for adapter hydration and GraphQL batching.
    pub fn limits(&self) -> &Upstream {
        &self.limits
    }
    pub fn max_pages(&self) -> u32 {
        self.limits.max_pages
    }
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        self.get_with_metadata(path, params)
            .await
            .map(|(value, _)| value)
    }
    async fn get_with_metadata<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<(T, HeaderMap)> {
        let mut url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::upstream())?;
        if url.origin() != self.base.origin() {
            return Err(Error::upstream());
        }
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let cached = self.cache.lock().await.get(url.as_str()).cloned();
        let mut headers = HeaderMap::new();
        if let Some(c) = &cached {
            headers.insert(http::header::IF_NONE_MATCH, c.etag.clone());
        }
        for attempt in 0..2 {
            let bucket = self.bucket(&url);
            let _permit = self.budget.acquire(bucket).await?;
            let future = self.raw_get(&url, headers.clone());
            let (status, response_headers, bytes) =
                tokio::time::timeout(Duration::from_secs(self.limits.timeout_seconds), future)
                    .await
                    .map_err(|_| Error::upstream())??;
            if status == StatusCode::NOT_MODIFIED {
                let cached = cached.as_ref().ok_or_else(Error::upstream)?;
                return serde_json::from_slice(&cached.body)
                    .map(|value| (value, cached.headers.clone()))
                    .map_err(|_| Error::upstream());
            }
            if status.is_server_error() && attempt == 0 {
                drop(_permit);
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            if !status.is_success() {
                if status == StatusCode::NOT_FOUND {
                    return Err(Error::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_not_found",
                        "The upstream resource was not found.",
                    ));
                }
                return Err(Error::upstream());
            }
            let value = serde_json::from_slice(&bytes).map_err(|_| Error::upstream())?;
            if bytes.len() <= 16 * 1024 * 1024
                && let Some(etag) = response_headers.get(http::header::ETAG)
            {
                let mut cache = self.cache.lock().await;
                let used: usize = cache.values().map(|c| c.body.len()).sum();
                if cache.len() >= 2048 || used.saturating_add(bytes.len()) > 16 * 1024 * 1024 {
                    cache.clear();
                }
                cache.insert(
                    url.to_string(),
                    Conditional {
                        etag: etag.clone(),
                        body: bytes,
                        headers: response_headers.clone(),
                    },
                );
            }
            return Ok((value, response_headers));
        }
        Err(Error::upstream())
    }
    async fn raw_get(
        &self,
        url: &url::Url,
        headers: HeaderMap,
    ) -> Result<(StatusCode, HeaderMap, Bytes)> {
        let mut request = self.client.get(url.clone()).headers(headers);
        if self.kind == Kind::Github {
            request = request.header(http::header::CONTENT_LENGTH, "0");
        }
        let response = request.send().await.map_err(|_| Error::upstream())?;
        self.read_response(self.bucket(url), response).await
    }
    async fn read_response(
        &self,
        bucket: Bucket,
        response: reqwest::Response,
    ) -> Result<(StatusCode, HeaderMap, Bytes)> {
        let status = response.status();
        let headers = response.headers().clone();
        // Publish before reading any bytes so queued callers see header backoff.
        let rate = self.budget.observe_headers(bucket, status, &headers);
        if self.kind != Kind::Github
            && let Some(error) = rate
        {
            return Err(error);
        }
        // GitHub may report exhausted primary headers and a secondary/abuse
        // limit in the same body. Inspect it to upgrade to origin-wide backoff,
        // retaining the header-derived error if the bounded body read fails.
        let bytes = bounded_body(response, self.limits.max_response_bytes)
            .await
            .map_err(|error| rate.clone().unwrap_or(error))?;
        if let Some(error) = self
            .budget
            .observe_body(bucket, status, &headers, &bytes)
            .or(rate)
        {
            return Err(error);
        }
        Ok((status, headers, bytes))
    }
    fn bucket(&self, url: &url::Url) -> Bucket {
        if self.kind == Kind::Github {
            if url
                .path()
                .strip_prefix(self.base.path())
                .unwrap_or(url.path())
                .starts_with("search/")
            {
                Bucket::GithubSearch
            } else {
                Bucket::GithubCore
            }
        } else {
            Bucket::General
        }
    }
    /// GitHub's POST-only GraphQL endpoint is restricted here to query documents.
    pub async fn query<T: DeserializeOwned, V: Serialize>(
        &self,
        query: &str,
        variables: V,
    ) -> Result<T> {
        if !query.trim_start().starts_with("query ") || query.contains("mutation") {
            return Err(Error::upstream());
        }
        if self.kind != Kind::Github {
            return Err(Error::upstream());
        }
        let payload = serde_json::json!({"query":query,"variables":variables});
        let url = self
            .base
            .join(if self.base.path().ends_with("/api/v3/") {
                "../graphql"
            } else {
                "graphql"
            })
            .map_err(|_| Error::upstream())?;
        if url.origin() != self.base.origin() {
            return Err(Error::upstream());
        }
        let bucket = Bucket::GithubGraphql;
        let _permit = self.budget.acquire(bucket).await?;
        let (status, _, bytes) =
            tokio::time::timeout(Duration::from_secs(self.limits.timeout_seconds), async {
                let response = self
                    .client
                    .post(url)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|_| Error::upstream())?;
                self.read_response(bucket, response).await
            })
            .await
            .map_err(|_| Error::upstream())??;
        if !status.is_success() {
            return Err(Error::upstream());
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::upstream())?;
        serde_json::from_value(value).map_err(|_| Error::upstream())
    }
    pub async fn pages<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Vec<T>> {
        self.collect_pages(path, params, None).await
    }
    /// An unannotated short page is not proof of completion: servers may clamp
    /// page size. Assigned issue inventories continue to an empty page instead.
    /// A stable item key makes reported totals count unique issues despite overlap.
    pub async fn complete_pages<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
        key: impl Fn(&T) -> Result<String> + Send + Sync,
    ) -> Result<Vec<T>> {
        self.collect_pages(path, params, Some(&key)).await
    }
    async fn collect_pages<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
        key: Option<PageKey<'_, T>>,
    ) -> Result<Vec<T>> {
        let mut items = Vec::new();
        let mut seen = BTreeSet::new();
        let mut expected_total = None;
        for page in 1..=self.max_pages() {
            let mut p = params.to_vec();
            p.push(("page", page.to_string()));
            p.push(("per_page", self.page_size().to_string()));
            p.push(("limit", self.page_size().to_string()));
            let (batch, headers): (Vec<T>, HeaderMap) = self.get_with_metadata(path, &p).await?;
            let has_next = if let Some(next) = headers.get("x-next-page") {
                Some(!next.is_empty())
            } else if headers.contains_key(http::header::LINK) {
                Some(
                    headers
                        .get_all(http::header::LINK)
                        .iter()
                        .filter_map(|v| v.to_str().ok())
                        .any(|v| {
                            v.split(',').any(|part| {
                                part.contains("rel=\"next\"") || part.contains("rel=next")
                            })
                        }),
                )
            } else {
                None
            };
            let mut total = headers
                .get("x-total-count")
                .or_else(|| headers.get("x-total"))
                .map(|h| {
                    h.to_str()
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                        .ok_or_else(Error::upstream)
                })
                .transpose()?;
            if key.is_some() {
                // Some servers report the inventory total only on the first
                // page. A later omitted header cannot erase that obligation.
                expected_total = total.or(expected_total);
                total = expected_total;
            }
            // A repeated/overlapping page cannot count the same upstream items
            // toward a total twice. Keep overlap in the returned inventory; the
            // adapter deduplicates its source/repository/item keys before hydration.
            let received = if let Some(key) = key {
                for item in &batch {
                    seen.insert(key(item)?);
                }
                seen.len()
            } else {
                items.len().saturating_add(batch.len())
            };
            let done = has_next.map(|next| !next).unwrap_or_else(|| {
                if let Some(total) = total {
                    received >= total
                } else if key.is_some() {
                    batch.is_empty()
                } else {
                    batch.len() < usize::try_from(self.page_size()).unwrap_or(usize::MAX)
                }
            });
            if done && total.is_some_and(|total| received < total) {
                return Err(Error::upstream());
            }
            if !done && batch.is_empty() {
                return Err(Error::upstream());
            }
            items.extend(batch);
            if done {
                return Ok(items);
            }
        }
        Err(Error::new(
            StatusCode::BAD_GATEWAY,
            "pagination_limit",
            "Upstream pagination exceeded the configured bound; previous data was retained.",
        ))
    }
}

/// Configuration-only GET helper for Jira discovery. No writes, cache or server.
/// Uses the same bounded client/body and origin admission/backoff as forge reads.
pub struct ReadHttp {
    client: reqwest::Client,
    base: url::Url,
    limits: Upstream,
    budget: Arc<RequestBudget>,
    basic_auth: Option<(String, zeroize::Zeroizing<String>)>,
}

/// A safe, local diagnostic. Backend REST reads keep their existing static errors.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ReadDiagnosticError {
    pub status: Option<StatusCode>,
    message: String,
}

fn read_diagnostic(status: Option<StatusCode>, message: impl Into<String>) -> anyhow::Error {
    ReadDiagnosticError {
        status,
        message: message.into(),
    }
    .into()
}

impl ReadHttp {
    /// Create a GET-only client rooted at the configured HTTP(S) base path.
    /// Headers can contain credentials; they are marked sensitive and never logged.
    /// Construction does not probe the network. Redirects and automatic retries are off.
    pub fn new(base_url: &str, mut headers: HeaderMap, limits: &Upstream) -> anyhow::Result<Self> {
        limits.validate()?;
        let base = url::Url::parse(&format!("{}/", base_url.trim_end_matches('/')))
            .map_err(|_| anyhow::anyhow!("invalid upstream base URL"))?;
        anyhow::ensure!(
            http_url(base_url) && base.query().is_none() && base.fragment().is_none(),
            "invalid upstream base URL"
        );
        for value in headers.values_mut() {
            value.set_sensitive(true);
        }
        let client = http_client_builder(limits)
            .default_headers(headers)
            .build()
            .map_err(|_| anyhow::anyhow!("HTTP client initialization failed"))?;
        Ok(Self {
            client,
            budget: RequestBudget::shared(base.as_str(), limits)?,
            base,
            limits: limits.clone(),
            basic_auth: None,
        })
    }

    /// Use Jira's configured Basic-auth email/token. With default features,
    /// literal tokens resolve but keyring references fail explicitly, without fallback.
    pub async fn jira(config: &JiraConfig, limits: &Upstream) -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );
        let mut transport = Self::new(&config.base_url, headers, limits)?;
        transport.basic_auth = Some((config.email.clone(), config.token.resolve().await?));
        Ok(transport)
    }

    pub fn limits(&self) -> &Upstream {
        &self.limits
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        self.get_with_metadata(path, params)
            .await
            .map(|(value, _)| value)
    }

    /// Read JSON with actionable, secret-redacted errors for the local setup wizard.
    ///
    /// Uses the same GET-only client, origin checks, request budget and bounds as
    /// normal reads. Never includes a request URL, headers, raw body or raw error
    /// chain. Only Jira's recognized plain-text message fields may be displayed.
    pub async fn get_diagnostic<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> anyhow::Result<T> {
        let mut url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| read_diagnostic(None, "invalid upstream GET path"))?;
        anyhow::ensure!(
            url.origin() == self.base.origin()
                && url.path().starts_with(self.base.path())
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "upstream GET path must remain within the configured service"
        );
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let _permit = self.budget.acquire(Bucket::General).await.map_err(|e| {
            read_diagnostic(
                None,
                e.retry_at.map_or_else(
                    || "upstream request could not acquire its request budget".to_owned(),
                    |at| format!("upstream rate limit: retry after {}", at.to_rfc3339()),
                ),
            )
        })?;
        let timeout = Duration::from_secs(self.limits.timeout_seconds);
        tokio::time::timeout(timeout, async {
            let mut request = self.client.get(url);
            if let Some((email, token)) = &self.basic_auth {
                request = request.basic_auth(email, Some(token.as_str()));
            }
            let mut response = request.send().await.map_err(|e| {
                if e.is_timeout() {
                    self.diagnostic_timeout()
                } else if e.is_connect() {
                    read_diagnostic(
                        None,
                        "upstream connection failed; check DNS, TLS and network access",
                    )
                } else {
                    read_diagnostic(None, "upstream GET could not be sent")
                }
            })?;
            let status = response.status();
            let rate = self
                .budget
                .observe_headers(Bucket::General, status, response.headers());
            if !status.is_success() || rate.is_some() {
                let mut message = format!(
                    "upstream GET returned HTTP {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("unknown status")
                );
                if let Some(at) = rate.and_then(|e| e.retry_at) {
                    message.push_str(&format!("; rate limit: retry after {}", at.to_rfc3339()));
                }
                // No body is surfaced for arbitrary authenticated clients: only
                // Jira Basic-auth credentials have a complete redaction scope.
                if let Some((email, token)) = &self.basic_auth {
                    let limit = self.limits.max_response_bytes.min(64 * 1024);
                    match diagnostic_body(&mut response, limit).await {
                        Ok(bytes) => {
                            if let Some(detail) = jira_error_detail(&bytes, email, token) {
                                message.push_str(": ");
                                message.push_str(&detail);
                            }
                        }
                        Err(BodyFailure::TooLarge) => {
                            message.push_str("; error response exceeded the byte limit")
                        }
                        Err(BodyFailure::Timeout) => {
                            message.push_str("; reading the error response timed out")
                        }
                        Err(BodyFailure::Read) => {
                            message.push_str("; error response could not be read")
                        }
                    }
                }
                return Err(read_diagnostic(Some(status), message));
            }
            let bytes = diagnostic_body(&mut response, self.limits.max_response_bytes)
                .await
                .map_err(|e| match e {
                    BodyFailure::TooLarge => read_diagnostic(
                        Some(status),
                        format!(
                            "upstream JSON response exceeded the {} byte limit",
                            self.limits.max_response_bytes
                        ),
                    ),
                    BodyFailure::Timeout => self.diagnostic_timeout(),
                    BodyFailure::Read => {
                        read_diagnostic(Some(status), "upstream JSON response could not be read")
                    }
                })?;
            serde_json::from_slice(&bytes).map_err(|_| {
                read_diagnostic(
                    Some(status),
                    "upstream GET returned malformed or unexpected JSON",
                )
            })
        })
        .await
        .map_err(|_| self.diagnostic_timeout())?
    }

    fn diagnostic_timeout(&self) -> anyhow::Error {
        read_diagnostic(
            None,
            format!(
                "upstream GET timed out ({} second request limit; {} second connection limit)",
                self.limits.timeout_seconds,
                self.limits.timeout_seconds.min(10)
            ),
        )
    }

    /// Read one page; callers must explicitly follow complete pagination within
    /// `limits().max_pages` and fail closed on incomplete results.
    pub async fn get_with_metadata<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<(T, HeaderMap)> {
        let mut url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::upstream())?;
        if url.origin() != self.base.origin()
            || !url.path().starts_with(self.base.path())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::upstream());
        }
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let _permit = self.budget.acquire(Bucket::General).await?;
        tokio::time::timeout(Duration::from_secs(self.limits.timeout_seconds), async {
            let mut request = self.client.get(url);
            if let Some((email, token)) = &self.basic_auth {
                request = request.basic_auth(email, Some(token.as_str()));
            }
            let response = request.send().await.map_err(|_| Error::upstream())?;
            let status = response.status();
            let headers = response.headers().clone();
            if let Some(error) = self
                .budget
                .observe_headers(Bucket::General, status, &headers)
            {
                return Err(error);
            }
            if !status.is_success() {
                return Err(Error::upstream());
            }
            let bytes = bounded_body(response, self.limits.max_response_bytes).await?;
            let value = serde_json::from_slice(&bytes).map_err(|_| Error::upstream())?;
            Ok((value, headers))
        })
        .await
        .map_err(|_| Error::upstream())?
    }
}

enum BodyFailure {
    TooLarge,
    Timeout,
    Read,
}

async fn diagnostic_body(
    response: &mut reqwest::Response,
    limit: usize,
) -> std::result::Result<Vec<u8>, BodyFailure> {
    if response
        .content_length()
        .is_some_and(|len| len > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Err(BodyFailure::TooLarge);
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        if e.is_timeout() {
            BodyFailure::Timeout
        } else {
            BodyFailure::Read
        }
    })? {
        if data.len().saturating_add(chunk.len()) > limit {
            return Err(BodyFailure::TooLarge);
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

fn jira_error_detail(bytes: &[u8], email: &str, token: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut messages = Vec::new();
    if let Some(items) = value
        .get("errorMessages")
        .and_then(serde_json::Value::as_array)
    {
        messages.extend(items.iter().filter_map(serde_json::Value::as_str));
    }
    if let Some(items) = value.get("errors").and_then(serde_json::Value::as_object) {
        // Keys are untrusted too and may contain arbitrary request data.
        messages.extend(items.values().filter_map(serde_json::Value::as_str));
    }
    for key in ["message", "title", "detail"] {
        if let Some(item) = value.get(key).and_then(serde_json::Value::as_str) {
            messages.push(item);
        }
    }
    let credential = zeroize::Zeroizing::new(format!("{email}:{token}"));
    let mut redactions = Vec::new();
    for secret in [token, credential.as_str(), email] {
        if !secret.is_empty() {
            redactions.push(zeroize::Zeroizing::new(secret.to_owned()));
            let base64 = base64_diagnostic_secret(secret.as_bytes());
            redactions.push(zeroize::Zeroizing::new(base64.clone()));
            redactions.push(zeroize::Zeroizing::new(
                base64.trim_end_matches('=').to_owned(),
            ));
            let url_base64 = base64.replace('+', "-").replace('/', "_");
            redactions.push(zeroize::Zeroizing::new(url_base64.clone()));
            redactions.push(zeroize::Zeroizing::new(
                url_base64.trim_end_matches('=').to_owned(),
            ));
            let encoded: String = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
            redactions.push(zeroize::Zeroizing::new(encoded.clone()));
            // Percent escapes are case-insensitive, unlike token contents.
            let mut lower_escapes = encoded.into_bytes();
            for i in 0..lower_escapes.len().saturating_sub(2) {
                if lower_escapes[i] == b'%' {
                    lower_escapes[i + 1].make_ascii_lowercase();
                    lower_escapes[i + 2].make_ascii_lowercase();
                }
            }
            redactions.push(zeroize::Zeroizing::new(
                String::from_utf8(lower_escapes).ok()?,
            ));
        }
    }
    // Redact larger compounds before their components, before truncating output.
    redactions.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = Vec::new();
    for item in messages.into_iter().take(8) {
        let mut item = zeroize::Zeroizing::new(item.to_owned());
        // Normalize escaped/control-split text BEFORE secret matching, otherwise
        // stripping a control character could reconstruct a redacted credential.
        for _ in 0..2 {
            *item = decode_diagnostic_percent(&item);
        }
        *item = item.chars().filter(|c| diagnostic_character(*c)).collect();
        for secret in &redactions {
            *item = item.replace(secret.as_str(), "[redacted]");
        }
        // Do not display HTML, links, credential/header-shaped or opaque content.
        // These are diagnostics, not a channel for arbitrary upstream content.
        if item.contains(['<', '>']) {
            continue;
        }
        let item = item
            .split_whitespace()
            .map(|word| {
                let lower = word.to_ascii_lowercase();
                if lower.contains("://")
                    || lower.starts_with("www.")
                    || lower.contains('%')
                    || lower.contains("authorization")
                    || lower.contains("cookie")
                    || lower.contains("token=")
                    || lower.contains("password=")
                    || (word.len() >= 32
                        && !word
                            .contains(|c: char| !c.is_ascii_alphanumeric() && !"+/=_-".contains(c)))
                {
                    "[redacted]"
                } else {
                    word
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let item: String = item
            .chars()
            .filter(|c| diagnostic_character(*c))
            .take(240)
            .collect();
        if !item.is_empty() && !out.contains(&item) {
            out.push(item);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("; ").chars().take(800).collect())
    }
}

fn diagnostic_character(c: char) -> bool {
    !c.is_control()
        && !matches!(c, '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}

fn decode_diagnostic_percent(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let mut remaining = value.as_bytes();
    while let Some((&first, rest)) = remaining.split_first() {
        if first == b'%'
            && let [a, b, tail @ ..] = rest
            && let (Some(a), Some(b)) = (char::from(*a).to_digit(16), char::from(*b).to_digit(16))
            && let Ok(byte) = u8::try_from(a * 16 + b)
        {
            out.push(byte);
            remaining = tail;
            continue;
        }
        out.push(first);
        remaining = rest;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Kept local to diagnostic redaction: no encoded secret is logged or returned.
fn base64_diagnostic_secret(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = usize::from(chunk[0]);
        let b = usize::from(*chunk.get(1).unwrap_or(&0));
        let c = usize::from(*chunk.get(2).unwrap_or(&0));
        out.push(char::from(ALPHABET[a >> 2]));
        out.push(char::from(ALPHABET[((a & 3) << 4) | (b >> 4)]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHABET[((b & 15) << 2) | (c >> 6)])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHABET[c & 63])
        } else {
            '='
        });
    }
    out
}

pub async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Bytes> {
    if response
        .content_length()
        .is_some_and(|len| len > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Err(Error::upstream());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::upstream())? {
        if data.len().saturating_add(chunk.len()) > limit {
            return Err(Error::upstream());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(data))
}

pub fn rate_error(status: StatusCode, headers: &HeaderMap) -> Option<Error> {
    crate::request_budget::rate_error(status, headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::{Value, json};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    const DIAGNOSTIC_TOKEN: &str = "test-private/credential+=42";
    const DIAGNOSTIC_EMAIL: &str = "human@example.invalid";

    fn diagnostic_client(server: &MockServer, limits: &Upstream) -> ReadHttp {
        let mut client = ReadHttp::new(&server.uri(), HeaderMap::new(), limits).unwrap();
        client.basic_auth = Some((
            DIAGNOSTIC_EMAIL.into(),
            zeroize::Zeroizing::new(DIAGNOSTIC_TOKEN.into()),
        ));
        client
    }

    #[test]
    fn jira_diagnostics_preserve_causes_but_not_credentials_or_untrusted_bodies() {
        let basic =
            base64_diagnostic_secret(format!("{DIAGNOSTIC_EMAIL}:{DIAGNOSTIC_TOKEN}").as_bytes());
        let token_base64 = base64_diagnostic_secret(DIAGNOSTIC_TOKEN.as_bytes());
        let split_token = DIAGNOSTIC_TOKEN.replace("private", "pri\u{200b}vate");
        let encoded = DIAGNOSTIC_TOKEN
            .bytes()
            .map(|b| format!("%{b:02x}"))
            .collect::<String>();
        let doubly_encoded = encoded.replace('%', "%25");
        let body = serde_json::to_vec(&json!({
            "errorMessages": [format!("Permission to view boards denied. {DIAGNOSTIC_TOKEN} {DIAGNOSTIC_EMAIL} Basic {basic} {token_base64} {split_token} {encoded} {doubly_encoded}")],
            "errors": {"ignored-credential-key": "Check Browse Projects permission."},
            "message": "<html>should not display</html>",
            "detail": "see https://untrusted.invalid/?secret=something",
            "unrecognized": "not a message"
        }))
        .unwrap();
        let detail = jira_error_detail(&body, DIAGNOSTIC_EMAIL, DIAGNOSTIC_TOKEN).unwrap();
        assert!(detail.contains("Permission to view boards denied"));
        assert!(detail.contains("Browse Projects"));
        assert!(detail.contains("[redacted]"));
        for secret in [
            DIAGNOSTIC_TOKEN,
            DIAGNOSTIC_EMAIL,
            &basic,
            &token_base64,
            &encoded,
            &doubly_encoded,
        ] {
            assert!(!detail.contains(secret));
        }
        assert!(!detail.contains("https://"));
        assert!(!detail.contains("<html>"));
        assert!(!detail.contains("not a message"));
        assert!(!detail.contains('\u{200b}'));
        assert!(detail.chars().count() <= 800);
        assert!(
            jira_error_detail(b"<html>private</html>", DIAGNOSTIC_EMAIL, DIAGNOSTIC_TOKEN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn diagnostic_get_exposes_http_status_and_safe_jira_cause_without_retrying() {
        let server = MockServer::start().await;
        for status in [401, 403, 404, 500] {
            Mock::given(method("GET"))
                .and(path(format!("/failure/{status}")))
                .and(header("authorization", format!("Basic {}", base64_diagnostic_secret(format!("{DIAGNOSTIC_EMAIL}:{DIAGNOSTIC_TOKEN}").as_bytes()))))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({"errorMessages":[format!("Specific Jira permission failure: {DIAGNOSTIC_TOKEN}")]})))
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = diagnostic_client(&server, &Upstream::default());
        for status in [401, 403, 404, 500] {
            let error = client
                .get_diagnostic::<Value>(&format!("failure/{status}"), &[])
                .await
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<ReadDiagnosticError>()
                    .unwrap()
                    .status
                    .unwrap()
                    .as_u16(),
                status
            );
            let message = format!("{error:#}");
            assert!(message.contains(&format!("HTTP {status}")));
            assert!(message.contains("Specific Jira permission failure"));
            assert!(!message.contains(DIAGNOSTIC_TOKEN));
            assert!(!message.contains(&server.uri()));
        }
    }

    #[tokio::test]
    async fn diagnostic_get_respects_rate_limit_and_reports_retry_without_more_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/limited"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "120")
                    .set_body_json(json!({"message":"Too many requests"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = diagnostic_client(&server, &Upstream::default());
        let first = client
            .get_diagnostic::<Value>("limited", &[])
            .await
            .unwrap_err()
            .to_string();
        assert!(first.contains("HTTP 429"));
        assert!(first.contains("retry after"));
        assert!(first.contains("Too many requests"));
        let suppressed = client
            .get_diagnostic::<Value>("limited", &[])
            .await
            .unwrap_err()
            .to_string();
        assert!(suppressed.contains("rate limit"));
        assert!(suppressed.contains("retry after"));
    }

    #[tokio::test]
    async fn diagnostic_get_distinguishes_size_json_and_timeout_failures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/oversized"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(2048)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/malformed"))
            .respond_with(ResponseTemplate::new(200).set_body_string(DIAGNOSTIC_TOKEN))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(1500))
                    .set_body_json(json!({})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = diagnostic_client(
            &server,
            &Upstream {
                timeout_seconds: 1,
                max_response_bytes: 1024,
                ..Upstream::default()
            },
        );
        for (route, expected) in [
            ("oversized", "1024 byte limit"),
            ("malformed", "malformed or unexpected JSON"),
            ("slow", "timed out (1 second request limit"),
        ] {
            let error = client
                .get_diagnostic::<Value>(route, &[])
                .await
                .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains(DIAGNOSTIC_TOKEN));
        }
    }

    #[tokio::test]
    async fn diagnostic_get_does_not_follow_redirects_or_contact_untrusted_origins() {
        let trusted = MockServer::start().await;
        let untrusted = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&untrusted)
            .await;
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", untrusted.uri()))
            .expect(1)
            .mount(&trusted)
            .await;
        let client = diagnostic_client(&trusted, &Upstream::default());
        let error = client
            .get_diagnostic::<Value>("redirect", &[])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("HTTP 302"));
        assert!(
            client
                .get_diagnostic::<Value>(&untrusted.uri(), &[])
                .await
                .is_err()
        );
        assert!(
            client
                .get_diagnostic::<Value>("resource?secret=hidden", &[])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn normal_backend_read_keeps_existing_static_wire_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/failure"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"errorMessages":["specific upstream details"]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let error = diagnostic_client(&server, &Upstream::default())
            .get::<Value>("failure", &[])
            .await
            .unwrap_err();
        assert_eq!(error.code, "upstream_failure");
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(!error.message.contains("specific upstream details"));
    }

    #[tokio::test]
    async fn diagnostic_get_never_surfaces_bodies_from_arbitrary_header_authentication() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/failure"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({"message":DIAGNOSTIC_TOKEN})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static(DIAGNOSTIC_TOKEN),
        );
        let client = ReadHttp::new(&server.uri(), headers, &Upstream::default()).unwrap();
        let message = format!(
            "{:#}",
            client
                .get_diagnostic::<Value>("failure", &[])
                .await
                .unwrap_err()
        );
        assert!(message.contains("HTTP 403"));
        assert!(!message.contains(DIAGNOSTIC_TOKEN));
    }

    #[tokio::test]
    async fn diagnostic_get_keeps_http_cause_when_error_body_is_oversized_or_html() {
        let server = MockServer::start().await;
        for (route, body) in [
            ("oversized-error", "x".repeat(2048)),
            ("html-error", format!("<html>{DIAGNOSTIC_TOKEN}</html>")),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/{route}")))
                .respond_with(ResponseTemplate::new(403).set_body_string(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = diagnostic_client(
            &server,
            &Upstream {
                max_response_bytes: 1024,
                ..Upstream::default()
            },
        );
        for route in ["oversized-error", "html-error"] {
            let error = client
                .get_diagnostic::<Value>(route, &[])
                .await
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<ReadDiagnosticError>().unwrap().status,
                Some(StatusCode::FORBIDDEN)
            );
            let message = format!("{error:#}");
            assert!(message.contains("HTTP 403 Forbidden"));
            assert!(!message.contains(DIAGNOSTIC_TOKEN));
            assert!(!message.contains("<html>"));
            if route == "oversized-error" {
                assert!(message.contains("byte limit"));
            }
        }
    }

    #[test]
    fn retry_after_and_reset_are_honored() {
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from_static("120"));
        let e = rate_error(StatusCode::TOO_MANY_REQUESTS, &h).unwrap();
        assert!(e.retry_at.unwrap() > Utc::now() + chrono::Duration::seconds(115));
        h.clear();
        h.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        h.insert("x-ratelimit-reset", HeaderValue::from_static("9999999999"));
        assert!(rate_error(StatusCode::FORBIDDEN, &h).is_some());
        assert!(rate_error(StatusCode::UNAUTHORIZED, &h).is_none());
    }
}
