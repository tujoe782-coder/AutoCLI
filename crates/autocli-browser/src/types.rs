use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonCommand {
    pub id: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// CDP method name for 'cdp' action (e.g. 'Input.insertText') · hermesDr fork
    #[serde(skip_serializing_if = "Option::is_none", rename = "cdpMethod")]
    pub cdp_method: Option<String>,
    /// CDP method params for 'cdp' action · hermesDr fork
    #[serde(skip_serializing_if = "Option::is_none", rename = "cdpParams")]
    pub cdp_params: Option<Value>,
    /// File paths for 'set-file-input' action (kept for logging / debugging — actual transfer goes via fileBlobs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    /// Selector for 'set-file-input' action — drop-zone element (defaults to 'input[type="file"]' on extension side)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Inline file contents (base64) for 'set-file-input' action (S321 gh#34 · drag-drop pattern).
    /// Carries file bytes through to the page since CDP DOM.setFileInputFiles is silently no-op'd
    /// from MV3 chrome.debugger context — we bypass the file chooser entirely and dispatch
    /// synthetic drag-drop events with reconstructed File objects.
    #[serde(skip_serializing_if = "Option::is_none", rename = "fileBlobs")]
    pub file_blobs: Option<Vec<FileBlob>>,
    /// S322: cookie domain scope for `cookies` action (daemon refuses bulk dumps).
    /// Set to the target host (e.g. `www.topview.ai` or `.topview.ai` for wildcard).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// Inline file payload for upload via drag-drop. Sent in DaemonCommand.fileBlobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBlob {
    pub name: String,
    /// Base64-encoded file contents.
    pub b64: String,
    /// MIME type (e.g. "image/jpeg"). Guessed from filename extension by the sender.
    pub mime: String,
}

impl DaemonCommand {
    pub fn new(action: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            action: action.into(),
            code: None,
            url: None,
            workspace: None,
            tab_id: None,
            format: None,
            cdp_method: None,
            cdp_params: None,
            files: None,
            selector: None,
            file_blobs: None,
            domain: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn with_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    pub fn with_tab_id(mut self, tab_id: u64) -> Self {
        self.tab_id = Some(tab_id);
        self
    }

    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// hermesDr fork: set CDP method for 'cdp' action.
    pub fn with_cdp_method(mut self, method: impl Into<String>) -> Self {
        self.cdp_method = Some(method.into());
        self
    }

    /// hermesDr fork: set CDP params for 'cdp' action.
    pub fn with_cdp_params(mut self, params: Value) -> Self {
        self.cdp_params = Some(params);
        self
    }

    /// hermesDr fork: set file paths for 'set-file-input' action.
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = Some(files);
        self
    }

    /// hermesDr fork: set selector for 'set-file-input' action.
    pub fn with_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// hermesDr fork (S321 gh#34): set inline file blobs for 'set-file-input' drag-drop action.
    pub fn with_file_blobs(mut self, blobs: Vec<FileBlob>) -> Self {
        self.file_blobs = Some(blobs);
        self
    }

    /// S322: set cookie domain scope for `cookies` action.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }
}

/// Article payload returned by the extension's read-article action.
///
/// Mirrors the shape produced by Mozilla Readability (@mozilla/readability).
/// All string fields default to empty when absent so the CLI can format
/// safely without repeated `Option::as_deref().unwrap_or("")`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArticle {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub byline: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
    /// Cleaned article HTML (Readability output).
    #[serde(default)]
    pub content: String,
    /// Plain-text version of content.
    #[serde(default)]
    pub text_content: String,
    #[serde(default)]
    pub length: u64,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub site_name: Option<String>,
    #[serde(default)]
    pub published_time: Option<String>,
    /// Final URL after redirects (as seen by the extension).
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonResult {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonResult {
    pub fn success(id: String, data: Value) -> Self {
        Self {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failure(id: String, error: String) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(error),
        }
    }
}
