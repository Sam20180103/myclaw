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
        }
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

        let timeout = std::time::Duration::from_millis(self.config.github_timeout_ms);

        match action {
            "view" => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("Missing 'path' for view action".to_string()))?;

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

pub fn register_reach_tools(registry: &crate::ToolRegistry, config: Option<ReachConfig>) {
    let config = config.unwrap_or_default();
    registry.register(std::sync::Arc::new(WebReachTool::new(config.clone())));
    registry.register(std::sync::Arc::new(VideoReachTool::new(config.clone())));
    registry.register(std::sync::Arc::new(GitHubReachTool::new(config.clone())));
    registry.register(std::sync::Arc::new(FeedReachTool::new(config.clone())));
    registry.register(std::sync::Arc::new(TwitterReachTool::new(config.clone())));
    registry.register(std::sync::Arc::new(RedditReachTool::new(config.clone())));
    registry.register(std::sync::Arc::new(WeiboReachTool::new(config)));
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
        
        let limiter1 = manager.get_or_create("reddit");
        let limiter2 = manager.get_or_create("reddit");
        
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
    }
}
