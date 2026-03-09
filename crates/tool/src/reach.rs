use crate::{Tool, ToolError, ToolSchema};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReachConfig {
    pub web_endpoint: String,
    pub web_timeout_ms: u64,
    pub video_timeout_ms: u64,
    pub github_timeout_ms: u64,
}

impl Default for ReachConfig {
    fn default() -> Self {
        Self {
            web_endpoint: "https://r.jina.ai".to_string(),
            web_timeout_ms: 30000,
            video_timeout_ms: 60000,
            github_timeout_ms: 30000,
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
    registry.register(std::sync::Arc::new(FeedReachTool::new(config)));
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
    fn test_reach_config_default() {
        let config = ReachConfig::default();
        assert_eq!(config.web_endpoint, "https://r.jina.ai");
        assert_eq!(config.web_timeout_ms, 30000);
    }

    #[test]
    fn test_register_reach_tools() {
        let registry = ToolRegistry::new();
        register_reach_tools(&registry, None);
        
        assert!(registry.get("web_reach").is_some());
        assert!(registry.get("video_reach").is_some());
        assert!(registry.get("github_reach").is_some());
        assert!(registry.get("feed_reach").is_some());
    }
}
