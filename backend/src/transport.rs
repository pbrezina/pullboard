//! Read-only forge transport, conditional requests, bounded pagination and backoff.
use crate::{
    config::{ForgeConfig, Upstream},
    error::{Error, Result},
    model::Kind,
    request_budget::{Bucket, RequestBudget},
};
use bytes::Bytes;
use gitlab::api::AsyncClient;
use http::{HeaderMap, HeaderValue, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

enum Client {
    Github(Box<octocrab::Octocrab>),
    Gitlab {
        token: HeaderValue,
        client: Box<SafeGitlab>,
    },
    Http(reqwest::Client),
}
// The SDK's built-in reqwest client follows redirects and cannot configure that
// policy. Keep its endpoint implementation but supply a bounded no-redirect
// implementation of the SDK's AsyncClient transport for authenticated reads.
struct SafeGitlab {
    sdk: gitlab::AsyncGitlab,
    http: reqwest::Client,
    limit: usize,
    budget: Arc<RequestBudget>,
}
impl gitlab::api::RestClient for SafeGitlab {
    type Error = Error;
    fn rest_endpoint(
        &self,
        endpoint: &str,
    ) -> std::result::Result<url::Url, gitlab::api::ApiError<Error>> {
        self.sdk
            .rest_endpoint(endpoint)
            .map_err(|_| gitlab::api::ApiError::client(Error::upstream()))
    }
    fn instance_endpoint(
        &self,
        endpoint: &str,
    ) -> std::result::Result<url::Url, gitlab::api::ApiError<Error>> {
        self.sdk
            .instance_endpoint(endpoint)
            .map_err(|_| gitlab::api::ApiError::client(Error::upstream()))
    }
}
#[async_trait::async_trait]
impl gitlab::api::AsyncClient for SafeGitlab {
    async fn rest_async(
        &self,
        request: http::request::Builder,
        body: Vec<u8>,
    ) -> std::result::Result<http::Response<Bytes>, gitlab::api::ApiError<Error>> {
        let call = async {
            let request = request.body(body).map_err(|_| Error::upstream())?;
            let request = reqwest::Request::try_from(request).map_err(|_| Error::upstream())?;
            let response = self
                .http
                .execute(request)
                .await
                .map_err(|_| Error::upstream())?;
            // Publish throttling before a bounded/failed/slow body can discard its
            // headers. General GitLab throttles need no body inspection; return
            // the sanitized error immediately and wake shared-origin waiters.
            if let Some(error) =
                self.budget
                    .observe_headers(Bucket::General, response.status(), response.headers())
            {
                return Err(error);
            }
            let mut out = http::Response::builder().status(response.status());
            *out.headers_mut().ok_or_else(Error::upstream)? = response.headers().clone();
            out.body(bounded_body(response, self.limit).await?)
                .map_err(|_| Error::upstream())
        };
        call.await.map_err(gitlab::api::ApiError::client)
    }
}
#[derive(Clone)]
struct Conditional {
    etag: HeaderValue,
    body: Bytes,
    headers: HeaderMap,
}
pub struct ReadTransport {
    client: Client,
    base: url::Url,
    limits: Upstream,
    cache: Mutex<BTreeMap<String, Conditional>>,
    budget: Arc<RequestBudget>,
}
type PageKey<'a, T> = &'a (dyn Fn(&T) -> Result<String> + Send + Sync);

pub fn http_client(limits: &Upstream) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("PullBoard/0.1")
        .timeout(Duration::from_secs(limits.timeout_seconds))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow::anyhow!("HTTP client initialization failed"))
}

impl ReadTransport {
    pub async fn new(config: &ForgeConfig, limits: &Upstream) -> anyhow::Result<Arc<Self>> {
        let secret = config.token.resolve().await?;
        let mut base = url::Url::parse(&format!("{}/", config.base_url.trim_end_matches('/')))
            .map_err(|_| anyhow::anyhow!("invalid forge base URL"))?;
        // Adapter API suffixes do not change the origin used by this budget.
        let budget = RequestBudget::shared(base.as_str(), limits)?;
        let client = match config.kind {
            Kind::Github => Client::Github(Box::new(
                octocrab::Octocrab::builder()
                    .base_uri(base.as_str())
                    .map_err(|_| anyhow::anyhow!("invalid GitHub API base URL"))?
                    .personal_token(secret.to_string())
                    .set_connect_timeout(Some(Duration::from_secs(10)))
                    .set_read_timeout(Some(Duration::from_secs(limits.timeout_seconds)))
                    .build()
                    .map_err(|_| anyhow::anyhow!("GitHub client initialization failed"))?,
            )),
            Kind::Gitlab => {
                let host = base
                    .as_str()
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_owned();
                // The crate's authenticated builder performs a hidden /user probe
                // and discards rate-limit headers on failure. Build without that
                // probe; supply the sensitive token on our explicit GET request.
                let mut builder = gitlab::GitlabBuilder::new_unauthenticated(host);
                if base.scheme() == "http" {
                    builder.insecure();
                }
                let client = builder
                    .build_async()
                    .await
                    .map_err(|_| anyhow::anyhow!("GitLab client initialization failed"))?;
                let mut token = HeaderValue::from_str(&secret)
                    .map_err(|_| anyhow::anyhow!("invalid GitLab token"))?;
                token.set_sensitive(true);
                base = base
                    .join("api/v4/")
                    .map_err(|_| anyhow::anyhow!("invalid GitLab API base URL"))?;
                Client::Gitlab {
                    token,
                    client: Box::new(SafeGitlab {
                        sdk: client,
                        http: http_client(limits)?,
                        limit: limits.max_response_bytes,
                        budget: budget.clone(),
                    }),
                }
            }
            Kind::Codeberg | Kind::Forgejo => {
                base = base
                    .join("api/v1/")
                    .map_err(|_| anyhow::anyhow!("invalid Forgejo API base URL"))?;
                let mut headers = HeaderMap::new();
                let mut value = HeaderValue::from_str(&format!("token {}", *secret))
                    .map_err(|_| anyhow::anyhow!("invalid forge token"))?;
                value.set_sensitive(true);
                headers.insert(http::header::AUTHORIZATION, value);
                let client = reqwest::Client::builder()
                    .default_headers(headers)
                    .user_agent("PullBoard/0.1")
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(limits.timeout_seconds))
                    .build()
                    .map_err(|_| anyhow::anyhow!("forge client initialization failed"))?;
                Client::Http(client)
            }
            Kind::Jira => anyhow::bail!("Jira is not a forge"),
        };
        Ok(Arc::new(Self {
            client,
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
        let bucket = self.bucket(url);
        let result = match &self.client {
            Client::Github(client) => {
                let response = client
                    ._get_with_headers(url.as_str(), Some(headers))
                    .await
                    .map_err(|_| Error::upstream())?;
                let (parts, body) = response.into_parts();
                let rate = self
                    .budget
                    .observe_headers(bucket, parts.status, &parts.headers);
                let bytes = Limited::new(body, self.limits.max_response_bytes)
                    .collect()
                    .await
                    .map_err(|_| rate.clone().unwrap_or_else(Error::upstream))?
                    .to_bytes();
                (parts.status, parts.headers, bytes, rate)
            }
            Client::Gitlab { token, client } => {
                let mut request = http::Request::builder()
                    .method("GET")
                    .uri(url.as_str())
                    .header("PRIVATE-TOKEN", token);
                for (name, value) in &headers {
                    request = request.header(name, value);
                }
                let response =
                    client
                        .rest_async(request, vec![])
                        .await
                        .map_err(|error| match error {
                            gitlab::api::ApiError::Client { source } => source,
                            _ => Error::upstream(),
                        })?;
                let (parts, bytes) = response.into_parts();
                // SafeGitlab already observed these headers before consuming bytes.
                (parts.status, parts.headers, bytes, None)
            }
            Client::Http(client) => {
                let response = client
                    .get(url.clone())
                    .headers(headers)
                    .send()
                    .await
                    .map_err(|_| Error::upstream())?;
                let status = response.status();
                let headers = response.headers().clone();
                let rate = self.budget.observe_headers(bucket, status, &headers);
                (
                    status,
                    headers,
                    bounded_body(response, self.limits.max_response_bytes)
                        .await
                        .map_err(|error| rate.clone().unwrap_or(error))?,
                    rate,
                )
            }
        };
        if result.2.len() > self.limits.max_response_bytes {
            return Err(result.3.unwrap_or_else(Error::upstream));
        }
        if let Some(error) = self
            .budget
            .observe_body(bucket, result.0, &result.1, &result.2)
            .or(result.3)
        {
            return Err(error);
        }
        Ok((result.0, result.1, result.2))
    }
    fn bucket(&self, url: &url::Url) -> Bucket {
        if matches!(self.client, Client::Github(_)) {
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
        let Client::Github(client) = &self.client else {
            return Err(Error::upstream());
        };
        let payload = serde_json::json!({"query":query,"variables":variables});
        let url = self
            .base
            .join(if self.base.path().ends_with("/api/v3/") {
                "../graphql"
            } else {
                "graphql"
            })
            .map_err(|_| Error::upstream())?;
        let bucket = Bucket::GithubGraphql;
        let _permit = self.budget.acquire(bucket).await?;
        let (status, response_headers, bytes, rate) =
            tokio::time::timeout(Duration::from_secs(self.limits.timeout_seconds), async {
                let response = client
                    ._post(url.as_str(), Some(&payload))
                    .await
                    .map_err(|_| Error::upstream())?;
                let status = response.status();
                let headers = response.headers().clone();
                let rate = self.budget.observe_headers(bucket, status, &headers);
                let bytes = Limited::new(response.into_body(), self.limits.max_response_bytes)
                    .collect()
                    .await
                    .map_err(|_| rate.clone().unwrap_or_else(Error::upstream))?
                    .to_bytes();
                Ok::<_, Error>((status, headers, bytes, rate))
            })
            .await
            .map_err(|_| Error::upstream())??;
        if let Some(error) = self
            .budget
            .observe_body(bucket, status, &response_headers, &bytes)
            .or(rate)
        {
            return Err(error);
        }
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
