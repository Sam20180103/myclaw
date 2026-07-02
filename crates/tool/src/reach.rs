use crate::{Tool, ToolError, ToolSchema};
use async_trait::async_trait;
use governor::{clock::DefaultClock, Quota, RateLimiter};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::num::NonZeroU32;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub burst_capacity: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 30,
            burst_capacity: 10,
        }
    }
}

#[allow(clippy::type_complexity)]
pub struct RateLimiterManager {
    limiters: RwLock<HashMap<String, Arc<RateLimiter<governor::state::NotKeyed, governor::state::InMemoryState, DefaultClock>>>>,
    default_config: RateLimitConfig,
}

impl Default for RateLimiterManager {
    fn default() -> Self {
        Self::new(RateLimitConfig::default())
    }
}

impl RateLimiterManager {
    pub fn new(default_config: RateLimitConfig) -> Self {
        Self {
            limiters: RwLock::new(HashMap::new()),
            default_config,
        }
    }

    pub fn get_or_create(&self, platform: &str) -> Arc<RateLimiter<governor::state::NotKeyed, governor::state::InMemoryState, DefaultClock>> {
        {
            let limiters = self.limiters.read();
            if let Some(limiter) = limiters.get(platform) {
                return limiter.clone();
            }
        }

        let config = self.get_platform_config(platform);
        let quota = Quota::per_minute(NonZeroU32::new(config.requests_per_minute).unwrap())
            .allow_burst(NonZeroU32::new(config.burst_capacity).unwrap());

        let limiter = Arc::new(RateLimiter::direct(quota));

        {
            let mut limiters = self.limiters.write();
            limiters.insert(platform.to_string(), limiter.clone());
        }

        limiter
    }

    pub fn check_rate_limit(&self, platform: &str) -> Result<(), ToolError> {
        let limiter = self.get_or_create(platform);
        match limiter.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(ToolError::ExecutionFailed(
                "Rate limit exceeded. Please wait before making more requests.".to_string(),
            )),
        }
    }

    fn get_platform_config(&self, platform: &str) -> RateLimitConfig {
        match platform {
            "twitter" => RateLimitConfig {
                requests_per_minute: 30,
                burst_capacity: 10,
            },
            "reddit" => RateLimitConfig {
                requests_per_minute: 60,
                burst_capacity: 20,
            },
            "weibo" => RateLimitConfig {
                requests_per_minute: 30,
                burst_capacity: 10,
            },
            "web" => RateLimitConfig {
                requests_per_minute: 120,
                burst_capacity: 30,
            },
            "github" => RateLimitConfig {
                requests_per_minute: 60,
                burst_capacity: 20,
            },
            _ => self.default_config.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReachConfig {
pub web_endpoint: String,
pub web_timeout_ms: u64,
pub video_timeout_ms: u64,
pub github_timeout_ms: u64,
pub social_timeout_ms: u64,
pub reddit_user_agent: String,
pub weibo_api_base: String,
pub mcp_endpoint: String,
pub linkedin_mcp_endpoint: String,
pub wechat_mcp_endpoint: String,
pub boss_mcp_endpoint: String,
pub podcast_mcp_endpoint: String,
pub boss_cookie: Option<String>,
pub exa_api_key: Option<String>,
pub xhs_mcp_endpoint: String,
pub xhs_cookie: Option<String>,
pub image_gen_base_url: Option<String>,
pub image_gen_api_key: Option<String>,
pub image_gen_model: Option<String>,
pub image_gen_output_dir: Option<String>,
}

impl Default for ReachConfig {
fn default() -> Self {
Self {
web_endpoint: "https://r.jina.ai".to_string(),
web_timeout_ms: 30000,
video_timeout_ms: 60000,
github_timeout_ms: 30000,
social_timeout_ms: 30000,
reddit_user_agent: "RustClaw/0.4.0".to_string(),
weibo_api_base: "https://weibo.com/ajax".to_string(),
mcp_endpoint: "http://127.0.0.1:8080".to_string(),
linkedin_mcp_endpoint: "http://127.0.0.1:8081".to_string(),
wechat_mcp_endpoint: "http://127.0.0.1:8082".to_string(),
boss_mcp_endpoint: "http://127.0.0.1:8083".to_string(),
podcast_mcp_endpoint: "http://127.0.0.1:8084".to_string(),
boss_cookie: None,
exa_api_key: None,
xhs_mcp_endpoint: "http://127.0.0.1:8085".to_string(),
xhs_cookie: None,
image_gen_base_url: None,
image_gen_api_key: None,
image_gen_model: None,
image_gen_output_dir: None,
}
}
}

pub struct McpClient {
    client: reqwest::Client,
    endpoint: String,
    #[allow(dead_code)]
    timeout_ms: u64,
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new("http://127.0.0.1:8080".to_string(), 30000)
    }
}

impl McpClient {
    pub fn new(endpoint: String, timeout_ms: u64) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms));

        // Local MCP bridge should not be routed via http_proxy/https_proxy.
        if endpoint.contains("127.0.0.1") || endpoint.contains("localhost") {
            builder = builder.no_proxy();
        }

        let client = builder
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, endpoint, timeout_ms }
    }

    pub async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let request_id = uuid::Uuid::new_v4().to_string();

        // 凭据保护: 过滤敏感参数用于日志
        let safe_params = Self::filter_sensitive_params(&params);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": request_id
        });

        tracing::debug!("MCP request: method={}, params={:?}", method, safe_params);

        let response = self
            .client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("MCP request failed: {}", e)))?;

        let status = response.status();
        let raw_body = response
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read MCP response body: {}", e)))?;

        if !status.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "MCP error: status={}, body={}",
                status,
                raw_body
            )));
        }

        let result: serde_json::Value = serde_json::from_str(&raw_body)
            .map_err(|e| ToolError::ExecutionFailed(format!(
                "Failed to parse MCP response: {}, raw_body={} ",
                e, raw_body
            )))?;

        if let Some(error) = result.get("error") {
            let error_msg = if let Some(msg) = error.get("message").and_then(|m| m.as_str()) {
                msg.to_string()
            } else {
                error.to_string()
            };
            return Err(ToolError::ExecutionFailed(format!(
                "MCP error: {} (full={})",
                error_msg,
                error
            )));
        }

        Ok(result.get("result").cloned().unwrap_or(result))
    }

    fn filter_sensitive_params(params: &serde_json::Value) -> serde_json::Value {
        let sensitive_keys = ["cookie", "password", "token", "api_key", "secret", "credential"];
        let mut filtered = params.clone();

        if let Some(obj) = filtered.as_object_mut() {
            for key in sensitive_keys {
                if obj.contains_key(key) {
                    obj.insert(key.to_string(), serde_json::Value::String("***REDACTED***".to_string()));
                }
            }
        }

        filtered
    }
}

pub struct WebReachTool {
    client: reqwest::Client,
    config: ReachConfig,
}

impl Default for WebReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl WebReachTool {
    pub fn new(config: ReachConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.web_timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }

    fn validate_url_against_ssrf(&self, url: &str) -> Result<(), ToolError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| ToolError::InvalidParams(format!("Invalid URL: {}", e)))?;

        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(ToolError::InvalidParams("Only http/https URLs allowed".to_string()));
        }

        if let Some(host) = parsed.host_str() {
            let blocked_hosts = [
                "localhost", "127.0.0.1", "0.0.0.0", "::1",
                "169.254.169.254",
                "10.", "192.168.", "172.16.", "172.17.", "172.18.", "172.19.",
                "172.20.", "172.21.", "172.22.", "172.23.", "172.24.", "172.25.",
                "172.26.", "172.27.", "172.28.", "172.29.", "172.30.", "172.31.",
                "::", "0:0:0:0:0:0:0:1",
            ];

            for blocked in &blocked_hosts {
                if host == *blocked || host.starts_with(blocked) {
                    return Err(ToolError::InvalidParams(
                        format!("Access to internal network resources is blocked: {}", host)
                    ));
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Tool for WebReachTool {
    fn name(&self) -> &str {
        "web_reach"
    }

    fn description(&self) -> &str {
        "Read and extract content from any webpage using Jina Reader API. Returns clean Markdown content."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_reach".to_string(),
            description: "Read webpage content via Jina Reader (free, no API key required)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to read (e.g., https://example.com)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'url' parameter".to_string()))?;

        // SSRF防护: 验证URL不指向内网资源
        self.validate_url_against_ssrf(url)?;

        let jina_url = format!("{}/{}", self.config.web_endpoint, url);

        let response = self
            .client
            .get(&jina_url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(ToolError::ExecutionFailed(format!(
                "HTTP error: {}",
                status
            )));
        }

        let content = response
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read response: {}", e)))?;

        Ok(serde_json::json!({
            "success": true,
            "url": url,
            "content": content,
            "format": "markdown"
        }))
    }
}

pub struct VideoReachTool {
    config: ReachConfig,
}

impl Default for VideoReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl VideoReachTool {
    pub fn new(config: ReachConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for VideoReachTool {
    fn name(&self) -> &str {
        "video_reach"
    }

    fn description(&self) -> &str {
        "Extract subtitles and metadata from YouTube/Bilibili videos using yt-dlp. Returns video title, description, and subtitles."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "video_reach".to_string(),
            description: "Extract video subtitles via yt-dlp (YouTube, Bilibili, etc.)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Video URL (YouTube or Bilibili)"
                    },
                    "extract_subtitles": {
                        "type": "boolean",
                        "description": "Whether to extract subtitles (default: true)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'url' parameter".to_string()))?;
        
        validate_url(url)?;

        let extract_subtitles = params
            .get("extract_subtitles")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let timeout = std::time::Duration::from_millis(self.config.video_timeout_ms);

        let mut cmd = tokio::process::Command::new("yt-dlp");
        cmd.arg("--dump-json");

        if extract_subtitles {
            cmd.arg("--write-sub").arg("--skip-download");
        }

        cmd.arg("--no-playlist").arg(url);

        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| ToolError::Timeout)?
            .map_err(|e| ToolError::ExecutionFailed(format!("yt-dlp failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ToolError::ExecutionFailed(format!(
                "yt-dlp error: {}",
                stderr
            )));
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let metadata: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

        Ok(serde_json::json!({
            "success": true,
            "url": url,
            "title": metadata.get("title").and_then(|v| v.as_str()).unwrap_or(""),
            "description": metadata.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "duration": metadata.get("duration").and_then(|v| v.as_u64()).unwrap_or(0),
            "uploader": metadata.get("uploader").and_then(|v| v.as_str()).unwrap_or(""),
            "subtitles_available": metadata.get("subtitles").is_some()
        }))
    }
}

pub struct GitHubReachTool {
    config: ReachConfig,
}

impl Default for GitHubReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl GitHubReachTool {
    pub fn new(config: ReachConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitHubReachTool {
    fn name(&self) -> &str {
        "github_reach"
    }

    fn description(&self) -> &str {
        "Read GitHub repository content, search code, and get repository information using gh CLI."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "github_reach".to_string(),
            description: "Access GitHub repos via gh CLI".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["view", "search", "info"],
                        "description": "Action to perform: view (read file), search (search code), info (repo info)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository in format 'owner/repo' (e.g., 'rust-lang/rust')"
                    },
                    "path": {
                        "type": "string",
                        "description": "File path for 'view' action"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for 'search' action"
                    }
                },
                "required": ["action", "repo"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let repo = params
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'repo' parameter".to_string()))?;
        
        validate_repo(repo)?;

        let timeout = std::time::Duration::from_millis(self.config.github_timeout_ms);

        match action {
            "view" => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'path' for view action".to_string()))?;
                
                validate_path(path)?;

                let output = tokio::time::timeout(
                    timeout,
                    tokio::process::Command::new("gh")
                        .arg("api")
                        .arg(format!("repos/{}/contents/{}", repo, path))
                        .output()
                )
                .await
                .map_err(|_| ToolError::Timeout)?
                .map_err(|e| ToolError::ExecutionFailed(format!("gh failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ToolError::ExecutionFailed(format!("gh error: {}", stderr)));
                }

                let response: serde_json::Value = serde_json::from_slice(&output.stdout)
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                if let Some(content_b64) = response.get("content").and_then(|v| v.as_str()) {
                    let content = base64_decode(content_b64);
                    Ok(serde_json::json!({
                        "success": true,
                        "repo": repo,
                        "path": path,
                        "content": content,
                        "encoding": "utf-8"
                    }))
                } else {
                    Ok(serde_json::json!({
                        "success": true,
                        "repo": repo,
                        "path": path,
                        "response": response
                    }))
                }
            }
            "search" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

                let output = tokio::time::timeout(
                    timeout,
                    tokio::process::Command::new("gh")
                        .arg("search")
                        .arg("code")
                        .arg(query)
                        .arg("--repo")
                        .arg(repo)
                        .arg("--json")
                        .arg("path,name,score")
                        .output()
                )
                .await
                .map_err(|_| ToolError::Timeout)?
                .map_err(|e| ToolError::ExecutionFailed(format!("gh failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ToolError::ExecutionFailed(format!("gh error: {}", stderr)));
                }

                let results: serde_json::Value = serde_json::from_slice(&output.stdout)
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                Ok(serde_json::json!({
                    "success": true,
                    "repo": repo,
                    "query": query,
                    "results": results
                }))
            }
            "info" => {
                let output = tokio::time::timeout(
                    timeout,
                    tokio::process::Command::new("gh")
                        .arg("repo")
                        .arg("view")
                        .arg(repo)
                        .arg("--json")
                        .arg("name,description,stargazersCount,forkCount,primaryLanguage,updatedAt")
                        .output()
                )
                .await
                .map_err(|_| ToolError::Timeout)?
                .map_err(|e| ToolError::ExecutionFailed(format!("gh failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ToolError::ExecutionFailed(format!("gh error: {}", stderr)));
                }

                let info: serde_json::Value = serde_json::from_slice(&output.stdout)
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                Ok(serde_json::json!({
                    "success": true,
                    "repo": repo,
                    "info": info
                }))
            }
            _ => Err(ToolError::InvalidParams(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }
}

pub struct FeedReachTool {
    client: reqwest::Client,
    #[allow(dead_code)]
    config: ReachConfig,
}

impl Default for FeedReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl FeedReachTool {
    pub fn new(config: ReachConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.web_timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }
}

#[derive(Debug, Serialize)]
pub struct FeedEntry {
    pub title: Option<String>,
    pub link: Option<String>,
    pub summary: Option<String>,
    pub published: Option<String>,
    pub author: Option<String>,
}

#[async_trait]
impl Tool for FeedReachTool {
    fn name(&self) -> &str {
        "feed_reach"
    }

    fn description(&self) -> &str {
        "Read and parse RSS/Atom feeds. Returns structured feed entries with title, link, summary, and publication date."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "feed_reach".to_string(),
            description: "Parse RSS/Atom feeds".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "RSS/Atom feed URL"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of entries to return (default: 10)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'url' parameter".to_string()))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(ToolError::ExecutionFailed(format!(
                "HTTP error: {}",
                status
            )));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read response: {}", e)))?;

        let feed = feed_rs::parser::parse(&content[..])
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse feed: {}", e)))?;

        let entries: Vec<FeedEntry> = feed
            .entries
            .into_iter()
            .take(limit)
            .map(|entry| FeedEntry {
                title: entry.title.map(|t| t.content),
                link: entry.links.first().map(|l| l.href.clone()),
                summary: entry.summary.map(|s| s.content),
                published: entry.published.map(|d| format!("{}", d)),
                author: entry.authors.first().map(|a| a.name.clone()),
            })
            .collect();

        Ok(serde_json::json!({
            "success": true,
            "url": url,
            "title": feed.title.map(|t| t.content),
            "entries": entries,
            "count": entries.len()
        }))
    }
}

fn base64_decode(input: &str) -> String {
    use base64::Engine;
    let input = input.replace('\n', "");
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

fn validate_url(url: &str) -> Result<(), ToolError> {
    let blocked_schemes = ["file://", "ftp://", "data:"];
    for scheme in blocked_schemes {
        if url.to_lowercase().starts_with(scheme) {
            return Err(ToolError::InvalidParams(format!("URL scheme '{}' is not allowed", scheme)));
        }
    }
    if url.contains('\0') {
        return Err(ToolError::InvalidParams("URL contains null byte".to_string()));
    }
    Ok(())
}

fn validate_repo(repo: &str) -> Result<(), ToolError> {
    if repo.is_empty() {
        return Err(ToolError::InvalidParams("Repository cannot be empty".to_string()));
    }
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(ToolError::InvalidParams("Repository must be in format 'owner/repo'".to_string()));
    }
    for part in parts {
        if part.is_empty() || part.contains('\0') || part.contains(' ') {
            return Err(ToolError::InvalidParams("Invalid repository name".to_string()));
        }
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), ToolError> {
    if path.contains('\0') {
        return Err(ToolError::InvalidParams("Path contains null byte".to_string()));
    }
    if path.contains("..") {
        return Err(ToolError::InvalidParams("Path traversal not allowed".to_string()));
    }
    Ok(())
}

/// 文生图工具 (OpenAI 兼容 /images/generations)。生成图片落盘到本地媒体目录,
/// 返回文件路径 (不把 base64 灌回 LLM 上下文); 路径可直接传给 xhs_reach publish。
pub struct ImageGenTool {
    config: ReachConfig,
    client: reqwest::Client,
}

impl Default for ImageGenTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl ImageGenTool {
    pub fn new(config: ReachConfig) -> Self {
        Self { config, client: reqwest::Client::new() }
    }

    fn output_dir(&self) -> std::path::PathBuf {
        if let Some(d) = &self.config.image_gen_output_dir {
            return std::path::PathBuf::from(d);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let root = std::path::Path::new(&home).join(".rustclaw");
        // G7: 与 Config::data_dir() 一致地按 profile 隔离 (tool crate 不依赖 config crate,故内联)
        let root = match std::env::var("RUSTCLAW_PROFILE") {
            Ok(p) if !p.trim().is_empty() && p.trim() != "default" => {
                root.join("profiles").join(p.trim())
            }
            _ => root,
        };
        root.join("media").join("generated")
    }
}

#[async_trait]
impl Tool for ImageGenTool {
    fn name(&self) -> &str {
        "image_gen"
    }

    fn description(&self) -> &str {
        "Generate images from a text prompt (OpenAI-compatible text-to-image). Saves PNG files locally and returns their paths, which can be passed to xhs_reach action=publish image_paths."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "image_gen".to_string(),
            description: "Text-to-image generation; returns local file paths".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "Image generation prompt" },
                    "n": { "type": "integer", "description": "Number of images (default 1)" },
                    "size": { "type": "string", "description": "Image size, e.g. 1024x1024 (default)" }
                },
                "required": ["prompt"]
            }),
        }
    }

    /// G4: 仅当文生图后端 (base_url + api_key) 已配置时可用。
    fn is_available(&self) -> bool {
        self.config.image_gen_base_url.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
            && self.config.image_gen_api_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        use base64::Engine as _;

        let base_url = self.config.image_gen_base_url.as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::ExecutionFailed("image_gen_base_url 未配置".to_string()))?;
        let api_key = self.config.image_gen_api_key.as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::ExecutionFailed("image_gen_api_key 未配置".to_string()))?;
        let model = self.config.image_gen_model.as_deref().unwrap_or("gpt-image-1");

        let prompt = params.get("prompt").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'prompt'".to_string()))?;
        let n = params.get("n").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 4);
        let size = params.get("size").and_then(|v| v.as_str()).unwrap_or("1024x1024");

        let url = format!("{}/images/generations", base_url.trim_end_matches('/'));
        let req_body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "n": n,
            "size": size,
            "response_format": "b64_json"
        });

        let resp = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&req_body)
            .send().await
            .map_err(|e| ToolError::ExecutionFailed(format!("image gen request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionFailed(format!("image gen HTTP {}: {}", status, body)));
        }

        let json: serde_json::Value = resp.json().await
            .map_err(|e| ToolError::ExecutionFailed(format!("image gen parse failed: {}", e)))?;

        let data = json.get("data").and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::ExecutionFailed("image gen: no 'data' in response".to_string()))?;

        let dir = self.output_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| ToolError::ExecutionFailed(format!("create output dir failed: {}", e)))?;

        let mut images = Vec::new();
        for item in data {
            let bytes = if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
                base64::engine::general_purpose::STANDARD.decode(b64)
                    .map_err(|e| ToolError::ExecutionFailed(format!("b64 decode failed: {}", e)))?
            } else if let Some(img_url) = item.get("url").and_then(|v| v.as_str()) {
                let r = self.client.get(img_url).send().await
                    .map_err(|e| ToolError::ExecutionFailed(format!("fetch image url failed: {}", e)))?;
                r.bytes().await
                    .map_err(|e| ToolError::ExecutionFailed(format!("read image bytes failed: {}", e)))?
                    .to_vec()
            } else {
                continue;
            };
            let path = dir.join(format!("{}.png", uuid::Uuid::new_v4()));
            std::fs::write(&path, &bytes)
                .map_err(|e| ToolError::ExecutionFailed(format!("write image failed: {}", e)))?;
            images.push(serde_json::json!({ "path": path.to_string_lossy(), "prompt": prompt }));
        }

        if images.is_empty() {
            return Err(ToolError::ExecutionFailed("image gen: no images returned".to_string()));
        }

        Ok(serde_json::json!({ "success": true, "count": images.len(), "images": images }))
    }
}

pub fn register_reach_tools(registry: &crate::ToolRegistry, config: Option<ReachConfig>) {
let config = config.unwrap_or_default();
registry.register(std::sync::Arc::new(WebReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(VideoReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(GitHubReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(FeedReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(TwitterReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(RedditReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(WeiboReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(XhsReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(ImageGenTool::new(config.clone())));
registry.register(std::sync::Arc::new(DouyinReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(SearchReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(LinkedInReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(WeChatReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(BossZhipinReachTool::new(config.clone())));
registry.register(std::sync::Arc::new(PodcastReachTool::new(config)));
}

pub struct TwitterReachTool {
    config: ReachConfig,
}

impl Default for TwitterReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl TwitterReachTool {
    pub fn new(config: ReachConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for TwitterReachTool {
    fn name(&self) -> &str {
        "twitter_reach"
    }

    fn description(&self) -> &str {
        "Read tweets, search Twitter/X, and get user timelines using xreach CLI or direct API."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "twitter_reach".to_string(),
            description: "Access Twitter/X data via xreach CLI or API".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["tweet", "user", "search", "timeline"],
                        "description": "Action: tweet (read tweet), user (user info), search (search tweets), timeline (user timeline)"
                    },
                    "tweet_id": {
                        "type": "string",
                        "description": "Tweet ID for 'tweet' action"
                    },
                    "username": {
                        "type": "string",
                        "description": "Username for 'user' or 'timeline' action"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for 'search' action"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of results (default: 20)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20);

        let timeout = std::time::Duration::from_millis(self.config.social_timeout_ms);

        match action {
            "tweet" => {
                let tweet_id = params
                    .get("tweet_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'tweet_id' for tweet action".to_string()))?;

                let url = format!("https://api.twitter.com/2/tweets/{}?tweet.fields=created_at,public_metrics,author_id", tweet_id);
                
                let output = tokio::time::timeout(
                    timeout,
                    tokio::process::Command::new("curl")
                        .arg("-s")
                        .arg("-H")
                        .arg("Content-Type: application/json")
                        .arg(&url)
                        .output()
                )
                .await
                .map_err(|_| ToolError::Timeout)?
                .map_err(|e| ToolError::ExecutionFailed(format!("curl failed: {}", e)))?;

                let response: serde_json::Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or_else(|_| serde_json::json!({"error": "Failed to parse response"}));

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": response
                }))
            }
            "search" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "query": query,
                    "note": "Twitter search requires API authentication. Use xreach CLI or configure API keys."
                }))
            }
            "user" | "timeline" => {
                let username = params
                    .get("username")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams(format!("Missing 'username' for {} action", action)))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "username": username,
                    "limit": limit,
                    "note": "Twitter user data requires API authentication. Use xreach CLI or configure API keys."
                }))
            }
            _ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
        }
    }
}

pub struct RedditReachTool {
    client: reqwest::Client,
    #[allow(dead_code)]
    config: ReachConfig,
}

impl Default for RedditReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl RedditReachTool {
    pub fn new(config: ReachConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.social_timeout_ms))
            .user_agent(&config.reddit_user_agent)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }
}

#[async_trait]
impl Tool for RedditReachTool {
    fn name(&self) -> &str {
        "reddit_reach"
    }

    fn description(&self) -> &str {
        "Read Reddit posts, comments, and search subreddits using the Reddit JSON API."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "reddit_reach".to_string(),
            description: "Access Reddit data via JSON API (no auth required)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["post", "subreddit", "search", "user"],
                        "description": "Action: post (read post), subreddit (list posts), search (search), user (user info)"
                    },
                    "url": {
                        "type": "string",
                        "description": "Full Reddit URL for 'post' action"
                    },
                    "subreddit": {
                        "type": "string",
                        "description": "Subreddit name for 'subreddit' action"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for 'search' action"
                    },
                    "username": {
                        "type": "string",
                        "description": "Username for 'user' action"
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["hot", "new", "top", "rising"],
                        "description": "Sort method for subreddit posts (default: hot)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of results (default: 25)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(25);

        match action {
            "post" => {
                let url = params
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'url' for post action".to_string()))?;

                let json_url = if url.ends_with(".json") { url.to_string() } else { format!("{}.json", url.trim_end_matches('/')) };

                let response = self
                    .client
                    .get(&json_url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

                if !response.status().is_success() {
                    return Err(ToolError::ExecutionFailed(format!("HTTP error: {}", response.status())));
                }

                let data: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": data
                }))
            }
            "subreddit" => {
                let subreddit = params
                    .get("subreddit")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'subreddit' for subreddit action".to_string()))?;

                let sort = params
                    .get("sort")
                    .and_then(|v| v.as_str())
                    .unwrap_or("hot");

                let url = format!(
                    "https://www.reddit.com/r/{}.json?sort={}&limit={}",
                    subreddit, sort, limit
                );

                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

                if !response.status().is_success() {
                    return Err(ToolError::ExecutionFailed(format!("HTTP error: {}", response.status())));
                }

                let data: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                let posts: Vec<serde_json::Value> = data["data"]["children"]
                    .as_array()
                    .map(|arr| arr.iter().map(|p| {
                        let pd = &p["data"];
                        serde_json::json!({
                            "id": pd["id"],
                            "title": pd["title"],
                            "author": pd["author"],
                            "score": pd["score"],
                            "url": pd["url"],
                            "selftext": pd["selftext"],
                            "created_utc": pd["created_utc"],
                        })
                    }).collect())
                    .unwrap_or_default();

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "subreddit": subreddit,
                    "sort": sort,
                    "posts": posts,
                    "count": posts.len()
                }))
            }
            "search" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

                let subreddit = params.get("subreddit").and_then(|v| v.as_str());

                let url = match subreddit {
                    Some(sr) => format!(
                        "https://www.reddit.com/r/{}/search.json?q={}&restrict_sr=1&limit={}",
                        sr, query, limit
                    ),
                    None => format!(
                        "https://www.reddit.com/search.json?q={}&limit={}",
                        query, limit
                    ),
                };

                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

                if !response.status().is_success() {
                    return Err(ToolError::ExecutionFailed(format!("HTTP error: {}", response.status())));
                }

                let data: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "query": query,
                    "subreddit": subreddit,
                    "data": data
                }))
            }
            "user" => {
                let username = params
                    .get("username")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'username' for user action".to_string()))?;

                let url = format!("https://www.reddit.com/user/{}/about.json", username);

                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

                if !response.status().is_success() {
                    return Err(ToolError::ExecutionFailed(format!("HTTP error: {}", response.status())));
                }

                let data: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": data
                }))
            }
            _ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
        }
    }
}

pub struct WeiboReachTool {
    client: reqwest::Client,
    config: ReachConfig,
}

impl Default for WeiboReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl WeiboReachTool {
    pub fn new(config: ReachConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.social_timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }
}

#[async_trait]
impl Tool for WeiboReachTool {
    fn name(&self) -> &str {
        "weibo_reach"
    }

    fn description(&self) -> &str {
        "Get Weibo hot searches, user status, and search content via Weibo API."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "weibo_reach".to_string(),
            description: "Access Weibo data via API".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["hot", "user", "search", "status"],
                        "description": "Action: hot (trending topics), user (user info), search (search), status (read post)"
                    },
                    "uid": {
                        "type": "string",
                        "description": "User ID for 'user' action"
                    },
                    "id": {
                        "type": "string",
                        "description": "Post ID for 'status' action"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for 'search' action"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of results (default: 20)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20);

        match action {
            "hot" => {
                let url = format!("{}/topicon/getTopIcon", self.config.weibo_api_base);
                
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("Request failed: {}", e)))?;

                let data: serde_json::Value = if response.status().is_success() {
                    response
                        .json()
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse JSON: {}", e)))?
                } else {
                    let backup_url = "https://weibo.com/ajax/side/hotSearch";
                    let backup_response = self
                        .client
                        .get(backup_url)
                        .send()
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(format!("Backup request failed: {}", e)))?;
                    
                    backup_response
                        .json()
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse backup JSON: {}", e)))?
                };

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": data,
                    "note": "Weibo hot search may require authentication for full data"
                }))
            }
            "user" => {
                let uid = params
                    .get("uid")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'uid' for user action".to_string()))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "uid": uid,
                    "note": "Weibo user data requires authentication. Configure cookies for full access."
                }))
            }
            "search" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "query": query,
                    "limit": limit,
                    "note": "Weibo search requires authentication. Configure cookies for access."
                }))
            }
            "status" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'id' for status action".to_string()))?;

                let _url = format!(
                    "{}/statuses/show?id={}",
                    self.config.weibo_api_base, id
                );

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "id": id,
                    "note": "Weibo status requires authentication. Configure cookies for access."
                }))
            }
            _ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
        }
    }
}

pub struct XhsReachTool {
    mcp_client: McpClient,
    #[allow(dead_code)]
    config: ReachConfig,
}

impl Default for XhsReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl XhsReachTool {
    pub fn new(config: ReachConfig) -> Self {
        // 登录/发布需较长时间, 取 max(120s, social_timeout)
        let timeout = config.social_timeout_ms.max(120_000);
        let mcp_client = McpClient::new(config.xhs_mcp_endpoint.clone(), timeout);
        Self { mcp_client, config }
    }

    /// 业务结果封装: 服务端在 cookie 缺失/失效时返回 status="auth_required"(login_method="sms"),
    /// 直接透传给 Agent 编排短信登录; 否则包装为成功结果。
    fn wrap_result(&self, action: &str, result: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        if result.get("status").and_then(|v| v.as_str()) == Some("auth_required") {
            let mut r = result;
            if let Some(obj) = r.as_object_mut() {
                obj.insert("action".to_string(), serde_json::Value::String(action.to_string()));
            }
            return r;
        }
        let mut base = serde_json::json!({ "success": true, "action": action, "data": result });
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    }
}

#[async_trait]
impl Tool for XhsReachTool {
    fn name(&self) -> &str {
        "xhs_reach"
    }

    fn description(&self) -> &str {
        "Xiaohongshu (小红书) via MCP bridge: read/search notes, list hot top-N, and publish a note. Login is phone+SMS two-step: on status=\"auth_required\" call action=login_send_sms, ask the user for the code, then login_submit_sms(code), and retry. To publish: write title/body/topics, generate images with the image_gen tool, then action=publish with image_paths."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "xhs_reach".to_string(),
            description: "Access Xiaohongshu data and publish notes via MCP bridge".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["note", "search", "user", "hot", "login_send_sms", "login_submit_sms", "publish"],
                        "description": "Action: note/search/user (read), hot (top-N hot notes), login_send_sms (send SMS to XHS_PHONE), login_submit_sms (submit code), publish (create+publish a note)"
                    },
                    "note_id": { "type": "string", "description": "Note ID for 'note' action" },
                    "query": { "type": "string", "description": "Search query for 'search' action" },
                    "user_id": { "type": "string", "description": "User ID for 'user' action" },
                    "limit": { "type": "integer", "description": "Result count (search default 20, hot default 10)" },
                    "phone": { "type": "string", "description": "Optional phone override for 'login_send_sms' (defaults to server XHS_PHONE)" },
                    "code": { "type": "string", "description": "SMS code for 'login_submit_sms' (from the user)" },
                    "title": { "type": "string", "description": "Note title for 'publish'" },
                    "body": { "type": "string", "description": "Note body text for 'publish'" },
                    "topics": { "type": "array", "items": { "type": "string" }, "description": "Hashtag topics for 'publish' (without #)" },
                    "image_paths": { "type": "array", "items": { "type": "string" }, "description": "Local image file paths for 'publish' (use image_gen to create them)" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20);

        match action {
            "note" => {
                let note_id = params
                    .get("note_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'note_id' for note action".to_string()))?;

                let result = self.mcp_client.call("xhs.get_note", serde_json::json!({
                    "note_id": note_id
                })).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": result
                }))
            }
            "search" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

                let result = self.mcp_client.call("xhs.search", serde_json::json!({
                    "query": query,
                    "limit": limit
                })).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "query": query,
                    "data": result
                }))
            }
            "user" => {
                let user_id = params
                    .get("user_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'user_id' for user action".to_string()))?;

                let result = self.mcp_client.call("xhs.get_user", serde_json::json!({
                    "user_id": user_id
                })).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": result
                }))
            }
            "hot" => {
                let hot_limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
                let cookie = self.config.xhs_cookie.as_deref().unwrap_or("");
                let result = self.mcp_client.call("xhs.get_hot", serde_json::json!({
                    "limit": hot_limit,
                    "cookie": cookie
                })).await?;
                Ok(self.wrap_result(action, result, serde_json::json!({ "limit": hot_limit })))
            }
            "login_send_sms" => {
                let mut call_params = serde_json::json!({});
                if let Some(phone) = params.get("phone").and_then(|v| v.as_str()) {
                    call_params["phone"] = serde_json::Value::String(phone.to_string());
                }
                let result = self.mcp_client.call("xhs.login_send_sms", call_params).await?;
                Ok(self.wrap_result(action, result, serde_json::json!({})))
            }
            "login_submit_sms" => {
                let code = params
                    .get("code")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'code' for login_submit_sms".to_string()))?;
                let result = self.mcp_client.call("xhs.login_submit_sms", serde_json::json!({
                    "code": code
                })).await?;
                Ok(self.wrap_result(action, result, serde_json::json!({})))
            }
            "publish" => {
                let title = params.get("title").and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'title' for publish".to_string()))?;
                let body = params.get("body").and_then(|v| v.as_str()).unwrap_or("");
                let topics = params.get("topics").cloned().unwrap_or_else(|| serde_json::json!([]));
                let image_paths = params.get("image_paths").cloned().unwrap_or_else(|| serde_json::json!([]));
                if image_paths.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                    return Err(ToolError::InvalidParams(
                        "publish 需要至少一张图片 (image_paths); 可先用 image_gen 生成".to_string(),
                    ));
                }
                let cookie = self.config.xhs_cookie.as_deref().unwrap_or("");
                let result = self.mcp_client.call("xhs.publish", serde_json::json!({
                    "title": title,
                    "body": body,
                    "topics": topics,
                    "image_paths": image_paths,
                    "cookie": cookie
                })).await?;
                Ok(self.wrap_result(action, result, serde_json::json!({ "title": title })))
            }
            _ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
        }
    }
}

pub struct DouyinReachTool {
    mcp_client: McpClient,
    #[allow(dead_code)]
    config: ReachConfig,
}

impl Default for DouyinReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl DouyinReachTool {
    pub fn new(config: ReachConfig) -> Self {
        let mcp_client = McpClient::new(config.mcp_endpoint.clone(), config.video_timeout_ms);
        Self { mcp_client, config }
    }
}

#[async_trait]
impl Tool for DouyinReachTool {
    fn name(&self) -> &str {
        "douyin_reach"
    }

    fn description(&self) -> &str {
        "Parse and download Douyin (TikTok China) videos via MCP bridge."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "douyin_reach".to_string(),
            description: "Access Douyin video data via MCP bridge".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["video", "user", "search"],
                        "description": "Action: video (parse video), user (user info), search (search videos)"
                    },
                    "url": {
                        "type": "string",
                        "description": "Video URL for 'video' action"
                    },
                    "user_id": {
                        "type": "string",
                        "description": "User ID for 'user' action"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for 'search' action"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        match action {
            "video" => {
                let url = params
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'url' for video action".to_string()))?;

                let result = self.mcp_client.call("douyin.parse", serde_json::json!({
                    "url": url
                })).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": result
                }))
            }
            "user" => {
                let user_id = params
                    .get("user_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'user_id' for user action".to_string()))?;

                let result = self.mcp_client.call("douyin.get_user", serde_json::json!({
                    "user_id": user_id
                })).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "data": result
                }))
            }
            "search" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

                let result = self.mcp_client.call("douyin.search", serde_json::json!({
                    "query": query
                })).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "query": query,
                    "data": result
                }))
            }
_ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
}
}
}

pub struct LinkedInReachTool {
mcp_client: McpClient,
#[allow(dead_code)]
config: ReachConfig,
}

impl Default for LinkedInReachTool {
fn default() -> Self {
Self::new(ReachConfig::default())
}
}

impl LinkedInReachTool {
pub fn new(config: ReachConfig) -> Self {
let mcp_client = McpClient::new(config.linkedin_mcp_endpoint.clone(), config.social_timeout_ms);
Self { mcp_client, config }
}
}

#[async_trait]
impl Tool for LinkedInReachTool {
fn name(&self) -> &str {
"linkedin_reach"
}

fn description(&self) -> &str {
"Access LinkedIn profiles, company pages, and job listings via MCP bridge."
}

fn schema(&self) -> ToolSchema {
ToolSchema {
name: "linkedin_reach".to_string(),
description: "Access LinkedIn data via MCP bridge".to_string(),
parameters: serde_json::json!({
"type": "object",
"properties": {
"action": {
"type": "string",
"enum": ["profile", "company", "job", "search"],
"description": "Action: profile (user profile), company (company page), job (job listing), search (search)"
},
"profile_id": {
"type": "string",
"description": "LinkedIn profile ID or URL for 'profile' action"
},
"company_id": {
"type": "string",
"description": "Company ID or name for 'company' action"
},
"job_id": {
"type": "string",
"description": "Job ID for 'job' action"
},
"query": {
"type": "string",
"description": "Search query for 'search' action"
},
"limit": {
"type": "integer",
"description": "Number of results (default: 20)"
}
},
"required": ["action"]
}),
}
}

async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
let action = params
.get("action")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

let limit = params
.get("limit")
.and_then(|v| v.as_u64())
.unwrap_or(20);

match action {
"profile" => {
let profile_id = params
.get("profile_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'profile_id' for profile action".to_string()))?;

let result = self.mcp_client.call("linkedin.get_profile", serde_json::json!({
"profile_id": profile_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
"company" => {
let company_id = params
.get("company_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'company_id' for company action".to_string()))?;

let result = self.mcp_client.call("linkedin.get_company", serde_json::json!({
"company_id": company_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
"job" => {
let job_id = params
.get("job_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'job_id' for job action".to_string()))?;

let result = self.mcp_client.call("linkedin.get_job", serde_json::json!({
"job_id": job_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
"search" => {
let query = params
.get("query")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

let result = self.mcp_client.call("linkedin.search", serde_json::json!({
"query": query,
"limit": limit
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"query": query,
"data": result
}))
}
_ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
}
}
}

pub struct WeChatReachTool {
mcp_client: McpClient,
#[allow(dead_code)]
config: ReachConfig,
}

impl Default for WeChatReachTool {
fn default() -> Self {
Self::new(ReachConfig::default())
}
}

impl WeChatReachTool {
pub fn new(config: ReachConfig) -> Self {
let mcp_client = McpClient::new(config.wechat_mcp_endpoint.clone(), config.social_timeout_ms);
Self { mcp_client, config }
}
}

#[async_trait]
impl Tool for WeChatReachTool {
fn name(&self) -> &str {
"wechat_reach"
}

fn description(&self) -> &str {
"Access WeChat Official Account articles and content via MCP bridge."
}

fn schema(&self) -> ToolSchema {
ToolSchema {
name: "wechat_reach".to_string(),
description: "Access WeChat Official Account data via MCP bridge".to_string(),
parameters: serde_json::json!({
"type": "object",
"properties": {
"action": {
"type": "string",
"enum": ["article", "search", "account"],
"description": "Action: article (read article), search (search articles), account (account info)"
},
"url": {
"type": "string",
"description": "Article URL for 'article' action"
},
"query": {
"type": "string",
"description": "Search query for 'search' action"
},
"account_id": {
"type": "string",
"description": "Official Account ID for 'account' action"
},
"limit": {
"type": "integer",
"description": "Number of results (default: 20)"
}
},
"required": ["action"]
}),
}
}

async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
let action = params
.get("action")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

let limit = params
.get("limit")
.and_then(|v| v.as_u64())
.unwrap_or(20);

match action {
"article" => {
let url = params
.get("url")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'url' for article action".to_string()))?;

let result = self.mcp_client.call("wechat.get_article", serde_json::json!({
"url": url
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
"search" => {
let query = params
.get("query")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

let result = self.mcp_client.call("wechat.search", serde_json::json!({
"query": query,
"limit": limit
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"query": query,
"data": result
}))
}
"account" => {
let account_id = params
.get("account_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'account_id' for account action".to_string()))?;

let result = self.mcp_client.call("wechat.get_account", serde_json::json!({
"account_id": account_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
_ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
}
}
}

pub struct BossZhipinReachTool {
mcp_client: McpClient,
#[allow(dead_code)]
config: ReachConfig,
}

impl Default for BossZhipinReachTool {
fn default() -> Self {
Self::new(ReachConfig::default())
}
}

impl BossZhipinReachTool {
pub fn new(config: ReachConfig) -> Self {
// Boss MCP often needs longer latency budget due to anti-bot challenge pages.
let boss_timeout_ms = config.social_timeout_ms.max(120_000);
let mcp_client = McpClient::new(config.boss_mcp_endpoint.clone(), boss_timeout_ms);
Self { mcp_client, config }
}

/// 业务结果封装。服务端在 cookie 缺失/失效时返回 status="auth_required"
/// (login_method="sms"),此时直接透传给 Agent,由 Agent 编排短信登录;
/// 否则包装为成功结果。`extra` 中的字段会并入成功响应顶层。
fn wrap_boss_result(&self, action: &str, result: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    if result.get("status").and_then(|v| v.as_str()) == Some("auth_required") {
        let mut r = result;
        if let Some(obj) = r.as_object_mut() {
            obj.insert("action".to_string(), serde_json::Value::String(action.to_string()));
        }
        return r;
    }
    let mut base = serde_json::json!({ "success": true, "action": action, "data": result });
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}
}

#[async_trait]
impl Tool for BossZhipinReachTool {
fn name(&self) -> &str {
"boss_zhipin_reach"
}

fn description(&self) -> &str {
    "Access Boss Zhipin (BOSS直聘) for job searching, retrieving job/company details, and applying for jobs. Login uses phone number + SMS code in two steps: when a business call returns status=\"auth_required\" (login_method=\"sms\"), call action=\"login_send_sms\" to send the code (server uses the configured BOSS_PHONE), ask the user for the code they received, then call action=\"login_submit_sms\" with code=... ; on success retry the business action. QR-code scan via Feishu remains only as an automatic fallback when SMS hits a slider captcha."
}
fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "boss_zhipin_reach".to_string(),
            description: "Access Boss Zhipin job data and perform recruitment actions via MCP bridge".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["search", "job", "company", "apply", "check_auth", "login_send_sms", "login_submit_sms", "get_qrcode", "wait_login"],
                        "description": "Action: search (search jobs), job (job detail), company (company info), apply (apply for job), check_auth (check login status), login_send_sms (send SMS code to BOSS_PHONE), login_submit_sms (submit the SMS code via 'code' param), get_qrcode (fallback: get QR code), wait_login (fallback: wait for scan login)"
                    },
                    "code": {
                        "type": "string",
                        "description": "SMS verification code for 'login_submit_sms' action (provided by the user)"
                    },
                    "phone": {
                        "type": "string",
                        "description": "Optional phone override for 'login_send_sms'; defaults to server BOSS_PHONE env"
                    },
                    "query": {
                        "type": "string",
                        "description": "Job search query for 'search' action"
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Job ID for 'job' or 'apply' action"
                    },
                    "company_id": {
                        "type": "string",
                        "description": "Company ID for 'company' action"
                    },
                    "city": {
                        "type": "string",
                        "description": "City filter for search (e.g., '北京', '上海')"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of results (default: 20)"
                    },
                    "resume_id": {
                        "type": "string",
                        "description": "Optional Resume ID for 'apply' action"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds for 'wait_login' action (default: 120)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);

    match action {
        "search" => {
            let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

        let city = params
            .get("city")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // cookie 可空: 服务端会复用已缓存 cookie, 失效则返回 auth_required(sms)
        let cookie = self.config.boss_cookie.as_deref().unwrap_or("");
        let result = self.mcp_client.call("boss.search_jobs", serde_json::json!({
            "query": query,
            "city": city,
            "limit": limit,
            "cookie": cookie
        })).await?;

            Ok(self.wrap_boss_result(action, result, serde_json::json!({ "query": query })))
        }
                "job" => {
                    let job_id = params
                        .get("job_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolError::InvalidParams("Missing 'job_id' for job action".to_string()))?;

                    let cookie = self.config.boss_cookie.as_deref().unwrap_or("");
                    let result = self.mcp_client.call("boss.get_job", serde_json::json!({
                        "job_id": job_id,
                        "cookie": cookie
                    })).await?;

                    Ok(self.wrap_boss_result(action, result, serde_json::json!({})))
                }
                "company" => {
                    let company_id = params
                        .get("company_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolError::InvalidParams("Missing 'company_id' for company action".to_string()))?;

                    let cookie = self.config.boss_cookie.as_deref().unwrap_or("");
                    let result = self.mcp_client.call("boss.get_company", serde_json::json!({
                        "company_id": company_id,
                        "cookie": cookie
                    })).await?;

                    Ok(self.wrap_boss_result(action, result, serde_json::json!({})))
                }
                "apply" => {
                    let job_id = params
                        .get("job_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolError::InvalidParams("Missing 'job_id' for apply action".to_string()))?;

                    let resume_id = params.get("resume_id").and_then(|v| v.as_str());

                    let cookie = self.config.boss_cookie.as_deref().unwrap_or("");
                    let result = self.mcp_client.call("boss.apply_job", serde_json::json!({
                        "job_id": job_id,
                        "resume_id": resume_id,
                        "cookie": cookie
                    })).await?;

                    Ok(self.wrap_boss_result(action, result, serde_json::json!({ "job_id": job_id })))
                }
                "check_auth" => {
                    let cookie = self.config.boss_cookie.as_deref().unwrap_or("");
                    let result = self.mcp_client.call("boss.check_auth", serde_json::json!({
                        "cookie": cookie
                    })).await?;

                    Ok(serde_json::json!({
                        "success": true,
                        "action": action,
                        "data": result
                    }))
                }
                "login_send_sms" => {
                    // 第一步: 服务端用 BOSS_PHONE 打开登录页并发送验证码
                    let mut call_params = serde_json::json!({});
                    if let Some(phone) = params.get("phone").and_then(|v| v.as_str()) {
                        call_params["phone"] = serde_json::Value::String(phone.to_string());
                    }
                    let result = self.mcp_client.call("boss.login_send_sms", call_params).await?;
                    Ok(self.wrap_boss_result(action, result, serde_json::json!({})))
                }
                "login_submit_sms" => {
                    // 第二步: 提交用户经飞书回复的验证码完成登录
                    let code = params
                        .get("code")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolError::InvalidParams("Missing 'code' for login_submit_sms action".to_string()))?;
                    let result = self.mcp_client.call("boss.login_submit_sms", serde_json::json!({
                        "code": code
                    })).await?;
                    Ok(self.wrap_boss_result(action, result, serde_json::json!({})))
                }
        "get_qrcode" => {
            // Phase 17.5: 获取登录二维码 (Base64图片)
            let result = self.mcp_client.call("boss.get_qrcode", serde_json::json!({})).await?;

            // 检查是否已登录
            if let Some(is_login) = result.get("is_login").and_then(|v| v.as_bool()) {
                if is_login {
                    return Ok(serde_json::json!({
                        "success": true,
                        "action": action,
                        "status": "already_logged_in",
                        "message": "已登录，无需扫码"
                    }));
                }
            }

            // 返回二维码图片数据
            let qrcode_base64 = result.get("qrcode_base64").and_then(|v| v.as_str()).unwrap_or("");
            let image_type = result.get("image_type").and_then(|v| v.as_str()).unwrap_or("png");
            let description = result.get("description").and_then(|v| v.as_str()).unwrap_or("请使用Boss直聘APP扫描此二维码登录");

            Ok(serde_json::json!({
                "success": true,
                "action": action,
                "status": "qrcode_ready",
                "qrcode_image": {
                    "image_data": qrcode_base64,
                    "image_type": image_type,
                    "description": description
                },
                "note": "请使用 rustclaw-protocol Content::Image 发送此二维码到飞书"
            }))
        }
        "wait_login" => {
            // Phase 17.5: 等待扫码登录
            let timeout = params
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(120) as i64;

            let result = self.mcp_client.call("boss.wait_login", serde_json::json!({
                "timeout": timeout
            })).await?;

            let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            let message = result.get("message").and_then(|v| v.as_str()).unwrap_or("");
            let cookie = result.get("cookie").and_then(|v| v.as_str()).unwrap_or("");

            if success {
                Ok(serde_json::json!({
                    "success": true,
                    "action": action,
                    "status": "login_success",
                    "message": message,
                    "cookie": cookie,
                    "note": "登录成功，返回了新 cookie，请更新 boss_cookie 配置"
                }))
            } else {
                Ok(serde_json::json!({
                    "success": false,
                    "action": action,
                    "status": "login_timeout",
                    "message": message
                }))
            }
        }
        _ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
    }
}
}
pub struct PodcastReachTool {
mcp_client: McpClient,
#[allow(dead_code)]
config: ReachConfig,
}

impl Default for PodcastReachTool {
fn default() -> Self {
Self::new(ReachConfig::default())
}
}

impl PodcastReachTool {
pub fn new(config: ReachConfig) -> Self {
let mcp_client = McpClient::new(config.podcast_mcp_endpoint.clone(), config.video_timeout_ms);
Self { mcp_client, config }
}
}

#[async_trait]
impl Tool for PodcastReachTool {
fn name(&self) -> &str {
"podcast_reach"
}

fn description(&self) -> &str {
"Access Xiaoyuzhou (小宇宙) podcast content and transcripts via MCP bridge."
}

fn schema(&self) -> ToolSchema {
ToolSchema {
name: "podcast_reach".to_string(),
description: "Access podcast data via MCP bridge".to_string(),
parameters: serde_json::json!({
"type": "object",
"properties": {
"action": {
"type": "string",
"enum": ["episode", "transcript", "search", "podcast"],
"description": "Action: episode (get episode), transcript (get transcript), search (search), podcast (podcast info)"
},
"episode_id": {
"type": "string",
"description": "Episode ID for 'episode' action"
},
"podcast_id": {
"type": "string",
"description": "Podcast ID for 'podcast' action"
},
"query": {
"type": "string",
"description": "Search query for 'search' action"
},
"limit": {
"type": "integer",
"description": "Number of results (default: 20)"
}
},
"required": ["action"]
}),
}
}

async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
let action = params
.get("action")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

let limit = params
.get("limit")
.and_then(|v| v.as_u64())
.unwrap_or(20);

match action {
"episode" => {
let episode_id = params
.get("episode_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'episode_id' for episode action".to_string()))?;

let result = self.mcp_client.call("podcast.get_episode", serde_json::json!({
"episode_id": episode_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
"transcript" => {
let episode_id = params
.get("episode_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'episode_id' for transcript action".to_string()))?;

let result = self.mcp_client.call("podcast.get_transcript", serde_json::json!({
"episode_id": episode_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
"search" => {
let query = params
.get("query")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'query' for search action".to_string()))?;

let result = self.mcp_client.call("podcast.search", serde_json::json!({
"query": query,
"limit": limit
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"query": query,
"data": result
}))
}
"podcast" => {
let podcast_id = params
.get("podcast_id")
.and_then(|v| v.as_str())
.ok_or_else(|| ToolError::InvalidParams("Missing 'podcast_id' for podcast action".to_string()))?;

let result = self.mcp_client.call("podcast.get_podcast", serde_json::json!({
"podcast_id": podcast_id
})).await?;

Ok(serde_json::json!({
"success": true,
"action": action,
"data": result
}))
}
_ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
}
}
}

pub struct SearchReachTool {
    client: reqwest::Client,
    config: ReachConfig,
}

impl Default for SearchReachTool {
    fn default() -> Self {
        Self::new(ReachConfig::default())
    }
}

impl SearchReachTool {
    pub fn new(config: ReachConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.social_timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }
}

#[async_trait]
impl Tool for SearchReachTool {
    fn name(&self) -> &str {
        "search_reach"
    }

    fn description(&self) -> &str {
        "Perform semantic search using Exa AI. Returns relevant web content based on natural language queries."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "search_reach".to_string(),
            description: "Semantic search via Exa AI".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query"
                    },
                    "num_results": {
                        "type": "integer",
                        "description": "Number of results (default: 10)"
                    },
                    "use_autoprompt": {
                        "type": "boolean",
                        "description": "Use Exa autoprompt (default: true)"
                    },
                    "category": {
                        "type": "string",
                        "enum": ["company", "research paper", "news", "github", "tweet", "movie", "song", "personal site", "pdf"],
                        "description": "Content category filter"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    /// G4: 仅当 Exa API key 已配置时可用。
    fn is_available(&self) -> bool {
        self.config.exa_api_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' parameter".to_string()))?;

        let num_results = params
            .get("num_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10);

        let use_autoprompt = params
            .get("use_autoprompt")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let api_key = match &self.config.exa_api_key {
            Some(key) => key.clone(),
            None => return Err(ToolError::ExecutionFailed(
                "Exa API key not configured. Set EXA_API_KEY environment variable.".to_string()
            )),
        };

        let mut body = serde_json::json!({
            "query": query,
            "numResults": num_results,
            "useAutoprompt": use_autoprompt,
        });

        if let Some(category) = params.get("category").and_then(|v| v.as_str()) {
            body["category"] = serde_json::json!(category);
        }

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Exa request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ToolError::ExecutionFailed(format!("Exa error: {}", response.status())));
        }

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse Exa response: {}", e)))?;

        Ok(serde_json::json!({
            "success": true,
            "query": query,
            "num_results": num_results,
            "results": result.get("results").cloned().unwrap_or_default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolRegistry;

    #[test]
    fn test_web_reach_tool_schema() {
        let tool = WebReachTool::default();
        assert_eq!(tool.name(), "web_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_video_reach_tool_schema() {
        let tool = VideoReachTool::default();
        assert_eq!(tool.name(), "video_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_github_reach_tool_schema() {
        let tool = GitHubReachTool::default();
        assert_eq!(tool.name(), "github_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_feed_reach_tool_schema() {
        let tool = FeedReachTool::default();
        assert_eq!(tool.name(), "feed_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_twitter_reach_tool_schema() {
        let tool = TwitterReachTool::default();
        assert_eq!(tool.name(), "twitter_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_reddit_reach_tool_schema() {
        let tool = RedditReachTool::default();
        assert_eq!(tool.name(), "reddit_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_weibo_reach_tool_schema() {
        let tool = WeiboReachTool::default();
        assert_eq!(tool.name(), "weibo_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_rate_limit_config_default() {
        let config = RateLimitConfig::default();
        assert_eq!(config.requests_per_minute, 30);
        assert_eq!(config.burst_capacity, 10);
    }

    #[test]
    fn test_rate_limiter_manager_creation() {
        let manager = RateLimiterManager::default();
        assert!(manager.limiters.read().is_empty());
    }

    #[test]
    fn test_rate_limiter_manager_get_or_create() {
        let manager = RateLimiterManager::default();
        
        let _limiter1 = manager.get_or_create("reddit");
        let _limiter2 = manager.get_or_create("reddit");
        
        assert_eq!(manager.limiters.read().len(), 1);
    }

    #[test]
    fn test_rate_limiter_manager_platform_configs() {
        let manager = RateLimiterManager::default();
        
        let reddit_config = manager.get_platform_config("reddit");
        assert_eq!(reddit_config.requests_per_minute, 60);
        assert_eq!(reddit_config.burst_capacity, 20);
        
        let twitter_config = manager.get_platform_config("twitter");
        assert_eq!(twitter_config.requests_per_minute, 30);
        assert_eq!(twitter_config.burst_capacity, 10);
        
        let unknown_config = manager.get_platform_config("unknown");
        assert_eq!(unknown_config.requests_per_minute, 30);
        assert_eq!(unknown_config.burst_capacity, 10);
    }

    #[test]
    fn test_rate_limiter_check() {
        let manager = RateLimiterManager::new(RateLimitConfig {
            requests_per_minute: 10,
            burst_capacity: 5,
        });
        
        for _ in 0..5 {
            assert!(manager.check_rate_limit("test").is_ok());
        }
    }

    #[test]
    fn test_reach_config_default() {
        let config = ReachConfig::default();
        assert_eq!(config.web_endpoint, "https://r.jina.ai");
        assert_eq!(config.web_timeout_ms, 30000);
        assert_eq!(config.social_timeout_ms, 30000);
    }

#[test]
fn test_register_reach_tools() {
let registry = ToolRegistry::new();
register_reach_tools(&registry, None);

assert!(registry.get("web_reach").is_some());
assert!(registry.get("video_reach").is_some());
assert!(registry.get("github_reach").is_some());
assert!(registry.get("feed_reach").is_some());
assert!(registry.get("twitter_reach").is_some());
assert!(registry.get("reddit_reach").is_some());
assert!(registry.get("weibo_reach").is_some());
assert!(registry.get("xhs_reach").is_some());
assert!(registry.get("douyin_reach").is_some());
assert!(registry.get("search_reach").is_some());
assert!(registry.get("linkedin_reach").is_some());
assert!(registry.get("wechat_reach").is_some());
assert!(registry.get("boss_zhipin_reach").is_some());
assert!(registry.get("podcast_reach").is_some());
}

    #[test]
    fn test_mcp_client_creation() {
        let client = McpClient::new("http://127.0.0.1:8080".to_string(), 30000);
        assert_eq!(client.endpoint, "http://127.0.0.1:8080");
    }

    #[test]
    fn test_xhs_reach_tool_schema() {
        let tool = XhsReachTool::default();
        assert_eq!(tool.name(), "xhs_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_douyin_reach_tool_schema() {
        let tool = DouyinReachTool::default();
        assert_eq!(tool.name(), "douyin_reach");
        let schema = tool.schema();
        assert!(schema.parameters.is_object());
    }

#[test]
fn test_search_reach_tool_schema() {
let tool = SearchReachTool::default();
assert_eq!(tool.name(), "search_reach");
let schema = tool.schema();
assert!(schema.parameters.is_object());
}

#[test]
fn test_linkedin_reach_tool_schema() {
let tool = LinkedInReachTool::default();
assert_eq!(tool.name(), "linkedin_reach");
let schema = tool.schema();
assert!(schema.parameters.is_object());
}

#[test]
fn test_wechat_reach_tool_schema() {
let tool = WeChatReachTool::default();
assert_eq!(tool.name(), "wechat_reach");
let schema = tool.schema();
assert!(schema.parameters.is_object());
}

#[test]
fn test_boss_zhipin_reach_tool_schema() {
let tool = BossZhipinReachTool::default();
assert_eq!(tool.name(), "boss_zhipin_reach");
let schema = tool.schema();
assert!(schema.parameters.is_object());
}

#[test]
fn test_podcast_reach_tool_schema() {
let tool = PodcastReachTool::default();
assert_eq!(tool.name(), "podcast_reach");
let schema = tool.schema();
assert!(schema.parameters.is_object());
}

#[test]
fn test_reach_config_mcp_fields() {
let config = ReachConfig::default();
assert_eq!(config.mcp_endpoint, "http://127.0.0.1:8080");
assert_eq!(config.linkedin_mcp_endpoint, "http://127.0.0.1:8081");
assert_eq!(config.wechat_mcp_endpoint, "http://127.0.0.1:8082");
assert_eq!(config.boss_mcp_endpoint, "http://127.0.0.1:8083");
assert_eq!(config.podcast_mcp_endpoint, "http://127.0.0.1:8084");
assert!(config.exa_api_key.is_none());
}

#[test]
fn test_register_reach_tools_phase14() {
let registry = ToolRegistry::new();
register_reach_tools(&registry, None);
assert!(registry.get("linkedin_reach").is_some());
assert!(registry.get("wechat_reach").is_some());
assert!(registry.get("boss_zhipin_reach").is_some());
assert!(registry.get("podcast_reach").is_some());
}

#[test]
fn test_reach_config_xhs_image_defaults() {
let c = ReachConfig::default();
assert_eq!(c.xhs_mcp_endpoint, "http://127.0.0.1:8085");
assert!(c.xhs_cookie.is_none());
assert!(c.image_gen_base_url.is_none());
assert!(c.image_gen_api_key.is_none());
}

#[test]
fn test_register_includes_image_gen() {
let registry = ToolRegistry::new();
register_reach_tools(&registry, None);
assert!(registry.get("image_gen").is_some());
assert!(registry.get("xhs_reach").is_some());
}

#[test]
fn test_xhs_publish_requires_images() {
let tool = XhsReachTool::default();
let rt = tokio::runtime::Runtime::new().unwrap();
let res = rt.block_on(tool.execute(serde_json::json!({ "action": "publish", "title": "t" })));
assert!(res.is_err(), "publish without image_paths should error");
}

#[test]
fn test_image_gen_requires_config() {
let tool = ImageGenTool::default();
let rt = tokio::runtime::Runtime::new().unwrap();
let res = rt.block_on(tool.execute(serde_json::json!({ "prompt": "a cat" })));
assert!(res.is_err(), "image_gen without base_url/api_key should error");
}

#[test]
fn test_g4_image_gen_availability() {
    use crate::Tool;
    // 未配置 → 不可用
    assert!(!ImageGenTool::default().is_available());
    // 配齐 base_url + api_key → 可用
    let cfg = ReachConfig {
        image_gen_base_url: Some("https://api.example.com/v1".to_string()),
        image_gen_api_key: Some("sk-x".to_string()),
        ..ReachConfig::default()
    };
    assert!(ImageGenTool::new(cfg).is_available());
}

#[test]
fn test_g4_search_reach_availability() {
    use crate::Tool;
    assert!(!SearchReachTool::default().is_available());
    let cfg = ReachConfig { exa_api_key: Some("exa-x".to_string()), ..ReachConfig::default() };
    assert!(SearchReachTool::new(cfg).is_available());
}

#[test]
fn test_g4_available_schemas_filters_unconfigured() {
    // 默认配置下 (无 exa / image_gen key),available_schemas 应少于全量 list
    let reg = crate::ToolRegistry::new();
    register_reach_tools(&reg, None);
    let all = reg.list().len();
    let available = reg.available_schemas().len();
    assert!(available < all, "available ({}) should be fewer than all ({})", available, all);
    // image_gen / search_reach 不应出现在可用列表
    let avail_names: Vec<String> = reg.available_schemas().into_iter().map(|s| s.name).collect();
    assert!(!avail_names.contains(&"image_gen".to_string()));
    assert!(!avail_names.contains(&"search_reach".to_string()));
}
}
