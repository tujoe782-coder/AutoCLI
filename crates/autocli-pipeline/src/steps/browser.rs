use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use autocli_core::{CliError, IPage, ScreenshotOptions, SnapshotOptions};
use serde_json::Value;

use crate::step_registry::{StepHandler, StepRegistry};
use crate::template::{render_template_str, TemplateContext};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn require_page(page: &Option<Arc<dyn IPage>>) -> Result<Arc<dyn IPage>, CliError> {
    page.clone()
        .ok_or_else(|| CliError::pipeline("browser step requires an active page"))
}

fn default_ctx(data: &Value, args: &HashMap<String, Value>) -> TemplateContext {
    TemplateContext {
        args: args.clone(),
        data: data.clone(),
        item: Value::Null,
        index: 0,
    }
}

fn render_str_param(
    params: &Value,
    data: &Value,
    args: &HashMap<String, Value>,
) -> Result<String, CliError> {
    let raw = params
        .as_str()
        .ok_or_else(|| CliError::pipeline("expected a string parameter"))?;
    let ctx = default_ctx(data, args);
    let rendered = render_template_str(raw, &ctx)?;
    rendered
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| CliError::pipeline("rendered template is not a string"))
}

// ---------------------------------------------------------------------------
// NavigateStep
// ---------------------------------------------------------------------------

pub struct NavigateStep;

#[async_trait]
impl StepHandler for NavigateStep {
    fn name(&self) -> &'static str {
        "navigate"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let ctx = default_ctx(data, args);

        let (url, settle_ms) = match params {
            // navigate: "https://example.com"
            Value::String(s) => {
                let rendered = render_template_str(s, &ctx)?;
                let url = rendered.as_str().unwrap_or("").to_string();
                (url, None)
            }
            // navigate: { url: "...", settleMs: 2000 }
            Value::Object(obj) => {
                let url_val = obj.get("url")
                    .ok_or_else(|| CliError::pipeline("navigate object requires 'url' field"))?;
                let url_str = url_val.as_str()
                    .ok_or_else(|| CliError::pipeline("navigate 'url' must be a string"))?;
                let rendered = render_template_str(url_str, &ctx)?;
                let url = rendered.as_str().unwrap_or("").to_string();
                let settle = obj.get("settleMs").and_then(|v| v.as_u64());
                (url, settle)
            }
            _ => return Err(CliError::pipeline("navigate expects a string URL or {url, settleMs} object")),
        };

        pg.goto(&url, None).await?;

        if let Some(ms) = settle_ms {
            // Explicit settleMs: use fixed wait
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        } else {
            // Auto-detect: wait for network idle + DOM stable
            let wait_js = r#"
                new Promise((resolve) => {
                    let lastActivity = Date.now();
                    let checkCount = 0;

                    // Monitor DOM changes
                    const observer = new MutationObserver(() => { lastActivity = Date.now(); });
                    observer.observe(document.body || document.documentElement, {
                        childList: true, subtree: true, attributes: true
                    });

                    // Monitor network via Performance API
                    let lastResourceCount = performance.getEntriesByType('resource').length;

                    const check = () => {
                        const now = Date.now();
                        const currentResources = performance.getEntriesByType('resource').length;
                        if (currentResources !== lastResourceCount) {
                            lastActivity = now;
                            lastResourceCount = currentResources;
                        }
                        checkCount++;
                        // Stable for 1.5s or timeout after 15s
                        if ((now - lastActivity > 1500 && checkCount > 5) || checkCount > 60) {
                            observer.disconnect();
                            resolve(true);
                        } else {
                            setTimeout(check, 250);
                        }
                    };
                    // Start checking after initial 500ms
                    setTimeout(check, 500);
                })
            "#;
            let _ = pg.evaluate(wait_js).await;
        }

        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// ClickStep
// ---------------------------------------------------------------------------

pub struct ClickStep;

#[async_trait]
impl StepHandler for ClickStep {
    fn name(&self) -> &'static str {
        "click"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let selector = render_str_param(params, data, args)?;
        pg.click(&selector).await?;
        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// TypeStep
// ---------------------------------------------------------------------------

pub struct TypeStep;

#[async_trait]
impl StepHandler for TypeStep {
    fn name(&self) -> &'static str {
        "type"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let ctx = default_ctx(data, args);

        let (selector, text) = match params {
            Value::Object(obj) => {
                let sel_raw = obj
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CliError::pipeline("type: missing 'selector' field"))?;
                let text_raw = obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CliError::pipeline("type: missing 'text' field"))?;

                let sel = render_template_str(sel_raw, &ctx)?;
                let txt = render_template_str(text_raw, &ctx)?;
                (
                    sel.as_str()
                        .ok_or_else(|| CliError::pipeline("type: rendered selector is not a string"))?
                        .to_string(),
                    txt.as_str()
                        .ok_or_else(|| CliError::pipeline("type: rendered text is not a string"))?
                        .to_string(),
                )
            }
            _ => return Err(CliError::pipeline("type: params must be an object with 'selector' and 'text'")),
        };

        pg.type_text(&selector, &text).await?;
        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// WaitStep
// ---------------------------------------------------------------------------

pub struct WaitStep;

#[async_trait]
impl StepHandler for WaitStep {
    fn name(&self) -> &'static str {
        "wait"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        _args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;

        match params {
            // wait: 2 (seconds — matching original opencli convention)
            Value::Number(n) => {
                let secs = n.as_f64().unwrap_or(1.0);
                let ms = (secs * 1000.0) as u64;
                pg.wait_for_timeout(ms).await?;
            }
            Value::Object(obj) => {
                if let Some(time_val) = obj.get("time") {
                    let secs = time_val.as_f64().unwrap_or(1.0);
                    let ms = (secs * 1000.0) as u64;
                    pg.wait_for_timeout(ms).await?;
                } else if let Some(sel_val) = obj.get("selector") {
                    let selector = sel_val
                        .as_str()
                        .ok_or_else(|| CliError::pipeline("wait: 'selector' must be a string"))?;
                    pg.wait_for_selector(selector, None).await?;
                } else if let Some(text_val) = obj.get("text") {
                    // Wait for text by using wait_for_selector with an XPath-like approach
                    // Since IPage doesn't have wait_for_text, we use evaluate in a polling loop
                    let text = text_val
                        .as_str()
                        .ok_or_else(|| CliError::pipeline("wait: 'text' must be a string"))?;
                    let js = format!(
                        r#"new Promise((resolve, reject) => {{
                            const timeout = setTimeout(() => reject(new Error('Timeout waiting for text')), 30000);
                            const check = () => {{
                                if (document.body.innerText.includes({})) {{
                                    clearTimeout(timeout);
                                    resolve(true);
                                }} else {{
                                    requestAnimationFrame(check);
                                }}
                            }};
                            check();
                        }})"#,
                        serde_json::to_string(text).unwrap_or_default()
                    );
                    pg.evaluate(&js).await?;
                } else {
                    return Err(CliError::pipeline(
                        "wait: object must have 'time', 'selector', or 'text'",
                    ));
                }
            }
            _ => return Err(CliError::pipeline("wait: params must be a number or object")),
        }

        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// PressStep
// ---------------------------------------------------------------------------

pub struct PressStep;

#[async_trait]
impl StepHandler for PressStep {
    fn name(&self) -> &'static str {
        "press"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let key = render_str_param(params, data, args)?;
        // Use evaluate to dispatch keyboard events since IPage has no press_key method
        let js = format!(
            r#"document.dispatchEvent(new KeyboardEvent('keydown', {{ key: {key}, bubbles: true }}));
               document.dispatchEvent(new KeyboardEvent('keyup', {{ key: {key}, bubbles: true }}));"#,
            key = serde_json::to_string(&key).unwrap_or_default()
        );
        pg.evaluate(&js).await?;
        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// EvaluateStep
// ---------------------------------------------------------------------------

pub struct EvaluateStep;

#[async_trait]
impl StepHandler for EvaluateStep {
    fn name(&self) -> &'static str {
        "evaluate"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let js = render_str_param(params, data, args)?;

        // Inject `args` and `data` as local variables so JS code can reference them
        // directly (e.g. `args.query`, `args.limit`) without ${{ }} template syntax.
        // This matches the original opencli behavior.
        let args_json = serde_json::to_string(args).unwrap_or("{}".to_string());
        let data_json = serde_json::to_string(data).unwrap_or("null".to_string());
        let wrapped_js = format!(
            "(function() {{ const args = {}; const data = {}; return ({}); }})()",
            args_json, data_json, js
        );

        let result = pg.evaluate(&wrapped_js).await?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// SnapshotStep
// ---------------------------------------------------------------------------

pub struct SnapshotStep;

#[async_trait]
impl StepHandler for SnapshotStep {
    fn name(&self) -> &'static str {
        "snapshot"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        _args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;

        let opts = match params {
            Value::Object(obj) => {
                let selector = obj.get("selector").and_then(|v| v.as_str()).map(String::from);
                let include_hidden = obj
                    .get("include_hidden")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Some(SnapshotOptions {
                    selector,
                    include_hidden,
                })
            }
            Value::Null => None,
            _ => None,
        };

        let result = pg.snapshot(opts).await?;
        if result.is_null() {
            Ok(data.clone())
        } else {
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
// ScreenshotStep
// ---------------------------------------------------------------------------

pub struct ScreenshotStep;

#[async_trait]
impl StepHandler for ScreenshotStep {
    fn name(&self) -> &'static str {
        "screenshot"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        _data: &Value,
        _args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;

        let opts = match params {
            Value::Object(obj) => {
                let full_page = obj
                    .get("full_page")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let selector = obj.get("selector").and_then(|v| v.as_str()).map(String::from);
                let path = obj.get("path").and_then(|v| v.as_str()).map(String::from);
                Some(ScreenshotOptions {
                    path,
                    full_page,
                    selector,
                })
            }
            Value::Null => None,
            _ => None,
        };

        let bytes = pg.screenshot(opts).await?;
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Value::String(b64))
    }
}

// ---------------------------------------------------------------------------
// ScrollStep
// ---------------------------------------------------------------------------

pub struct ScrollStep;

#[async_trait]
impl StepHandler for ScrollStep {
    fn name(&self) -> &'static str {
        "scroll"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;

        match params {
            // scroll: 3  (number of scrolls)
            Value::Number(n) => {
                let count = n.as_u64().unwrap_or(3) as u32;
                pg.auto_scroll(Some(autocli_core::AutoScrollOptions {
                    max_scrolls: Some(count),
                    delay_ms: Some(300),
                    ..Default::default()
                }))
                .await?;
            }
            // scroll: { direction: "down", count: 5, delay: 500 }
            Value::Object(obj) => {
                let count = obj
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3) as u32;
                let delay = obj
                    .get("delay")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(300);
                pg.auto_scroll(Some(autocli_core::AutoScrollOptions {
                    max_scrolls: Some(count),
                    delay_ms: Some(delay),
                    ..Default::default()
                }))
                .await?;
            }
            // scroll: "down" or template string
            Value::String(_) => {
                let ctx = default_ctx(data, args);
                let rendered = render_template_str(
                    params.as_str().unwrap_or("3"),
                    &ctx,
                )?;
                let count = rendered.as_u64().or_else(|| rendered.as_str().and_then(|s| s.parse().ok())).unwrap_or(3) as u32;
                pg.auto_scroll(Some(autocli_core::AutoScrollOptions {
                    max_scrolls: Some(count),
                    delay_ms: Some(300),
                    ..Default::default()
                }))
                .await?;
            }
            // scroll: null → default 3 scrolls
            _ => {
                pg.auto_scroll(Some(autocli_core::AutoScrollOptions {
                    max_scrolls: Some(3),
                    delay_ms: Some(300),
                    ..Default::default()
                }))
                .await?;
            }
        }

        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// CollectStep — collect intercepted requests and parse with JS function
// ---------------------------------------------------------------------------

pub struct CollectStep;

#[async_trait]
impl StepHandler for CollectStep {
    fn name(&self) -> &'static str {
        "collect"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        _data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;

        // Get the parse function from params
        let parse_fn = params
            .get("parse")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CliError::pipeline("collect step requires a 'parse' field with a JS function"))?;

        // Get intercepted data directly from browser (raw JSON, not typed structs)
        // and run the parse function on it — all in one evaluate call.
        let args_json = serde_json::to_string(args).unwrap_or("{}".to_string());
        let js = format!(
            r#"(() => {{
  const args = {args_json};
  const requests = window.__opencli_intercepted || [];
  window.__opencli_intercepted = [];
  const parseFn = {parse_fn};
  return parseFn(requests);
}})()"#
        );

        pg.evaluate(&js).await
    }
}

// ---------------------------------------------------------------------------
// CdpStep (hermesDr fork) — passthrough raw CDP commands (Input.insertText, Input.dispatchKeyEvent)
// ---------------------------------------------------------------------------

pub struct CdpStep;

#[async_trait]
impl StepHandler for CdpStep {
    fn name(&self) -> &'static str {
        "cdp"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let obj = params
            .as_object()
            .ok_or_else(|| CliError::pipeline("cdp: params must be an object with 'method' and optional 'params'"))?;
        let method = obj
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CliError::pipeline("cdp: missing 'method' field (e.g. 'Input.insertText')"))?
            .to_string();
        let cdp_params_raw = obj
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        // Template-expand string values inside cdp_params (so x/y can be `${{ data.x }}` etc)
        // After expansion, attempt numeric coercion (CDP expects ints for x/y/keyCode etc)
        let ctx = default_ctx(data, args);
        let cdp_params = render_value(&cdp_params_raw, &ctx)?;
        let result = pg.send_cdp(&method, cdp_params).await?;
        Ok(result)
    }
}

// Recursively walk a Value, render templates in string values, and coerce
// integer/float-looking strings back to numbers (CDP wire types require it).
fn render_value(v: &Value, ctx: &TemplateContext) -> Result<Value, CliError> {
    match v {
        Value::String(s) => {
            let rendered = render_template_str(s, ctx)?;
            // render_template_str returns a Value (could be String or other);
            // if it's a String, try numeric coercion
            if let Some(rs) = rendered.as_str() {
                if let Ok(n) = rs.parse::<i64>() { return Ok(serde_json::json!(n)); }
                if let Ok(f) = rs.parse::<f64>() { return Ok(serde_json::json!(f)); }
            }
            Ok(rendered)
        }
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, val) in map {
                new_map.insert(k.clone(), render_value(val, ctx)?);
            }
            Ok(Value::Object(new_map))
        }
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for val in arr {
                out.push(render_value(val, ctx)?);
            }
            Ok(Value::Array(out))
        }
        _ => Ok(v.clone()),
    }
}

// ---------------------------------------------------------------------------
// UploadFileStep (hermesDr fork · S321) — programmatic file upload via the
// extension's blessed `setFileInputFiles` helper. yaml usage:
//
//   - upload-file:
//       selector: 'input[data-s321-target="1"]'   # optional · defaults to input[type="file"]
//       files:
//         - "${{ args.file_path }}"               # ABSOLUTE paths reachable by Chrome
//
// Why a dedicated step instead of `cdp: { method: DOM.setFileInputFiles }`:
// the generic CDP passthrough does NOT call DOM.enable first (and DOM.enable is
// not in the extension's CDP_ALLOWLIST), so the DOM agent stays uninitialized
// and setFileInputFiles silently no-ops. The blessed extension helper handles
// DOM.enable internally and shares state across the chain.
// ---------------------------------------------------------------------------

pub struct UploadFileStep;

#[async_trait]
impl StepHandler for UploadFileStep {
    fn name(&self) -> &'static str {
        "upload-file"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let obj = params.as_object().ok_or_else(|| {
            CliError::pipeline("upload-file: params must be an object with 'files' (and optional 'selector')")
        })?;
        let ctx = default_ctx(data, args);

        let files_raw = obj
            .get("files")
            .ok_or_else(|| CliError::pipeline("upload-file: missing 'files' (array of paths)"))?;
        let files_arr = files_raw
            .as_array()
            .ok_or_else(|| CliError::pipeline("upload-file: 'files' must be an array"))?;
        if files_arr.is_empty() {
            return Err(CliError::pipeline("upload-file: 'files' array is empty"));
        }
        let mut files: Vec<String> = Vec::with_capacity(files_arr.len());
        for item in files_arr {
            let raw_str = item.as_str().ok_or_else(|| {
                CliError::pipeline("upload-file: each entry in 'files' must be a string path")
            })?;
            let rendered = render_template_str(raw_str, &ctx)?;
            let s = rendered
                .as_str()
                .ok_or_else(|| {
                    CliError::pipeline("upload-file: rendered file path is not a string")
                })?
                .to_string();
            if s.is_empty() {
                return Err(CliError::pipeline(
                    "upload-file: rendered file path is empty",
                ));
            }
            // CDP setFileInputFiles requires absolute paths that the Chrome process
            // can read. Hard-error early on relative or `~`-prefixed paths so the
            // user sees a clear failure rather than an extension-level silent fail.
            if !s.starts_with('/') {
                return Err(CliError::pipeline(format!(
                    "upload-file: path must be absolute (starts with '/'), got: {s}"
                )));
            }
            files.push(s);
        }

        let selector = if let Some(sel_raw) = obj.get("selector") {
            let sel_str = sel_raw
                .as_str()
                .ok_or_else(|| CliError::pipeline("upload-file: 'selector' must be a string"))?;
            let rendered = render_template_str(sel_str, &ctx)?;
            rendered.as_str().unwrap_or("").to_string()
        } else {
            String::new()
        };

        pg.set_file_input(&selector, files).await?;
        // Preserve `data` so subsequent steps that rely on a prior context aren't disrupted.
        Ok(data.clone())
    }
}

// ---------------------------------------------------------------------------
// UploadFileTrustedStep (hermesDr fork · S326) — trusted CDP file-chooser intercept
// + Input.dispatchMouseEvent click + DOM.setFileInputFiles. Supersedes the S321
// drag-drop sim path for sites whose React onChange handlers gate on isTrusted.
//
// Verified 2026-05-13 against Topview React 19 + Next.js 15 + Turbopack: trusted
// click pre-flight is REQUIRED (without it the React app crashes / resets); native
// file picker is intercepted (does not pop), and DOM.setFileInputFiles
// auto-fires `input` + `change` events with `isTrusted=true`, so React renders
// chip previews exactly as if a human had selected the file.
//
// yaml usage:
//
//   - upload-file-trusted:
//       selector: "button.border-dashed:has(svg.lucide-upload)"  # visible button to click
//       input_selector: "input[type=file]"                         # hidden file input to set
//       files:
//         - "${{ args.file_path_1 }}"                              # ABSOLUTE paths
//         - "${{ args.file_path_2 }}"                              # multiple=true supports N files in 1 call
//
// Trade-offs vs `upload-file:`:
//   • Pro: events become trusted → works on sites that reject synthetic drops
//   • Pro: file bytes do NOT cross daemon body limit (only path strings travel)
//   • Pro: multi-file in 1 call (no per-file repeats)
//   • Con: requires the visible button to be in viewport + clickable (selector + rect check)
// ---------------------------------------------------------------------------

pub struct UploadFileTrustedStep;

#[async_trait]
impl StepHandler for UploadFileTrustedStep {
    fn name(&self) -> &'static str {
        "upload-file-trusted"
    }

    fn is_browser_step(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        page: Option<Arc<dyn IPage>>,
        params: &Value,
        data: &Value,
        args: &HashMap<String, Value>,
    ) -> Result<Value, CliError> {
        let pg = require_page(&page)?;
        let obj = params.as_object().ok_or_else(|| {
            CliError::pipeline(
                "upload-file-trusted: params must be an object with 'selector', 'input_selector', and 'files'",
            )
        })?;
        let ctx = default_ctx(data, args);

        // button selector (REQUIRED — must trusted-click before setFileInputFiles)
        let selector_raw = obj.get("selector").ok_or_else(|| {
            CliError::pipeline(
                "upload-file-trusted: missing 'selector' (visible button to click)",
            )
        })?;
        let selector_str = selector_raw.as_str().ok_or_else(|| {
            CliError::pipeline("upload-file-trusted: 'selector' must be a string")
        })?;
        let selector = render_template_str(selector_str, &ctx)?
            .as_str()
            .unwrap_or("")
            .to_string();
        if selector.is_empty() {
            return Err(CliError::pipeline(
                "upload-file-trusted: rendered 'selector' is empty",
            ));
        }

        // input_selector (REQUIRED — hidden file input to set)
        let input_selector_raw = obj.get("input_selector").ok_or_else(|| {
            CliError::pipeline(
                "upload-file-trusted: missing 'input_selector' (hidden file input)",
            )
        })?;
        let input_selector_str = input_selector_raw.as_str().ok_or_else(|| {
            CliError::pipeline("upload-file-trusted: 'input_selector' must be a string")
        })?;
        let input_selector = render_template_str(input_selector_str, &ctx)?
            .as_str()
            .unwrap_or("")
            .to_string();
        if input_selector.is_empty() {
            return Err(CliError::pipeline(
                "upload-file-trusted: rendered 'input_selector' is empty",
            ));
        }

        // files (REQUIRED — array of absolute paths)
        let files_raw = obj.get("files").ok_or_else(|| {
            CliError::pipeline("upload-file-trusted: missing 'files' (array of absolute paths)")
        })?;
        let files_arr = files_raw.as_array().ok_or_else(|| {
            CliError::pipeline("upload-file-trusted: 'files' must be an array")
        })?;
        if files_arr.is_empty() {
            return Err(CliError::pipeline("upload-file-trusted: 'files' is empty"));
        }
        let mut files: Vec<String> = Vec::with_capacity(files_arr.len());
        for item in files_arr {
            let raw_str = item.as_str().ok_or_else(|| {
                CliError::pipeline(
                    "upload-file-trusted: each entry in 'files' must be a string path",
                )
            })?;
            let rendered = render_template_str(raw_str, &ctx)?;
            let s = rendered
                .as_str()
                .ok_or_else(|| {
                    CliError::pipeline("upload-file-trusted: rendered file path is not a string")
                })?
                .to_string();
            if s.is_empty() {
                return Err(CliError::pipeline(
                    "upload-file-trusted: rendered file path is empty",
                ));
            }
            if !s.starts_with('/') {
                return Err(CliError::pipeline(format!(
                    "upload-file-trusted: path must be absolute (starts with '/'), got: {s}"
                )));
            }
            files.push(s);
        }

        let result = pg
            .set_file_input_trusted(&selector, &input_selector, files)
            .await?;
        // Pass extension's verify payload (chipsRendered, chipLabels, filesSet) as step data.
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register_browser_steps(registry: &mut StepRegistry) {
    registry.register(Arc::new(NavigateStep));
    registry.register(Arc::new(ClickStep));
    registry.register(Arc::new(TypeStep));
    registry.register(Arc::new(WaitStep));
    registry.register(Arc::new(PressStep));
    registry.register(Arc::new(EvaluateStep));
    registry.register(Arc::new(SnapshotStep));
    registry.register(Arc::new(ScreenshotStep));
    registry.register(Arc::new(ScrollStep));
    registry.register(Arc::new(CollectStep));
    registry.register(Arc::new(CdpStep));  // hermesDr fork
    registry.register(Arc::new(UploadFileStep));  // hermesDr fork (S321)
    registry.register(Arc::new(UploadFileTrustedStep));  // hermesDr fork (S326)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use autocli_core::WaitOptions;
    use serde_json::json;

    fn empty_args() -> HashMap<String, Value> {
        HashMap::new()
    }

    // Mock IPage for testing
    struct MockPage {
        goto_url: std::sync::Mutex<Option<String>>,
        evaluate_result: Value,
    }

    impl MockPage {
        fn new(evaluate_result: Value) -> Self {
            Self {
                goto_url: std::sync::Mutex::new(None),
                evaluate_result,
            }
        }
    }

    #[async_trait]
    impl IPage for MockPage {
        async fn goto(
            &self,
            url: &str,
            _options: Option<autocli_core::GotoOptions>,
        ) -> Result<(), CliError> {
            *self.goto_url.lock().unwrap() = Some(url.to_string());
            Ok(())
        }
        async fn url(&self) -> Result<String, CliError> {
            Ok("https://example.com".to_string())
        }
        async fn title(&self) -> Result<String, CliError> {
            Ok("Mock".to_string())
        }
        async fn content(&self) -> Result<String, CliError> {
            Ok("<html></html>".to_string())
        }
        async fn evaluate(&self, _expression: &str) -> Result<Value, CliError> {
            Ok(self.evaluate_result.clone())
        }
        async fn wait_for_selector(
            &self,
            _selector: &str,
            _options: Option<WaitOptions>,
        ) -> Result<(), CliError> {
            Ok(())
        }
        async fn wait_for_navigation(
            &self,
            _options: Option<WaitOptions>,
        ) -> Result<(), CliError> {
            Ok(())
        }
        async fn wait_for_timeout(&self, _ms: u64) -> Result<(), CliError> {
            Ok(())
        }
        async fn click(&self, _selector: &str) -> Result<(), CliError> {
            Ok(())
        }
        async fn type_text(&self, _selector: &str, _text: &str) -> Result<(), CliError> {
            Ok(())
        }
        async fn cookies(
            &self,
            _options: Option<autocli_core::CookieOptions>,
        ) -> Result<Vec<autocli_core::Cookie>, CliError> {
            Ok(vec![])
        }
        async fn set_cookies(
            &self,
            _cookies: Vec<autocli_core::Cookie>,
        ) -> Result<(), CliError> {
            Ok(())
        }
        async fn screenshot(
            &self,
            _options: Option<ScreenshotOptions>,
        ) -> Result<Vec<u8>, CliError> {
            Ok(vec![0x89, 0x50, 0x4E, 0x47]) // PNG magic bytes
        }
        async fn snapshot(&self, _options: Option<SnapshotOptions>) -> Result<Value, CliError> {
            Ok(json!({"tree": "snapshot"}))
        }
        async fn auto_scroll(
            &self,
            _options: Option<autocli_core::AutoScrollOptions>,
        ) -> Result<(), CliError> {
            Ok(())
        }
        async fn tabs(&self) -> Result<Vec<autocli_core::TabInfo>, CliError> {
            Ok(vec![])
        }
        async fn switch_tab(&self, _tab_id: &str) -> Result<(), CliError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), CliError> {
            Ok(())
        }
        async fn intercept_requests(&self, _url_pattern: &str) -> Result<(), CliError> {
            Ok(())
        }
        async fn get_intercepted_requests(
            &self,
        ) -> Result<Vec<autocli_core::InterceptedRequest>, CliError> {
            Ok(vec![])
        }
        async fn get_network_requests(
            &self,
        ) -> Result<Vec<autocli_core::NetworkRequest>, CliError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_all_browser_steps_register() {
        let mut registry = StepRegistry::new();
        register_browser_steps(&mut registry);
        assert!(registry.get("navigate").is_some());
        assert!(registry.get("click").is_some());
        assert!(registry.get("type").is_some());
        assert!(registry.get("wait").is_some());
        assert!(registry.get("press").is_some());
        assert!(registry.get("evaluate").is_some());
        assert!(registry.get("snapshot").is_some());
        assert!(registry.get("screenshot").is_some());
    }

    #[tokio::test]
    async fn test_navigate_step() {
        let mock = Arc::new(MockPage::new(json!(null)));
        let step = NavigateStep;
        let result = step
            .execute(
                Some(mock.clone()),
                &json!("https://example.com"),
                &json!({"key": "value"}),
                &empty_args(),
            )
            .await
            .unwrap();
        assert_eq!(result, json!({"key": "value"}));
        assert_eq!(
            *mock.goto_url.lock().unwrap(),
            Some("https://example.com".to_string())
        );
    }

    #[tokio::test]
    async fn test_evaluate_step() {
        let mock = Arc::new(MockPage::new(json!({"items": [1, 2, 3]})));
        let step = EvaluateStep;
        let result = step
            .execute(
                Some(mock),
                &json!("document.querySelectorAll('.item')"),
                &json!(null),
                &empty_args(),
            )
            .await
            .unwrap();
        assert_eq!(result, json!({"items": [1, 2, 3]}));
    }

    #[tokio::test]
    async fn test_browser_step_requires_page() {
        let step = NavigateStep;
        let result = step
            .execute(None, &json!("https://example.com"), &json!(null), &empty_args())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_all_browser_steps_are_browser_steps() {
        assert!(NavigateStep.is_browser_step());
        assert!(ClickStep.is_browser_step());
        assert!(TypeStep.is_browser_step());
        assert!(WaitStep.is_browser_step());
        assert!(PressStep.is_browser_step());
        assert!(EvaluateStep.is_browser_step());
        assert!(SnapshotStep.is_browser_step());
        assert!(ScreenshotStep.is_browser_step());
    }

    #[tokio::test]
    async fn test_wait_step_with_time() {
        let mock = Arc::new(MockPage::new(json!(null)));
        let step = WaitStep;
        let result = step
            .execute(Some(mock), &json!(1000), &json!("data"), &empty_args())
            .await
            .unwrap();
        assert_eq!(result, json!("data"));
    }

    #[tokio::test]
    async fn test_snapshot_step() {
        let mock = Arc::new(MockPage::new(json!(null)));
        let step = SnapshotStep;
        let result = step
            .execute(Some(mock), &json!(null), &json!(null), &empty_args())
            .await
            .unwrap();
        assert_eq!(result, json!({"tree": "snapshot"}));
    }
}
