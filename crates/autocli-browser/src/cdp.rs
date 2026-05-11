use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use autocli_core::{
    AutoScrollOptions, CliError, Cookie, CookieOptions, GotoOptions, IPage, InterceptedRequest,
    NetworkRequest, ScreenshotOptions, SnapshotOptions, TabInfo, WaitOptions,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error};

use crate::dom_helpers;

type WsSink =
    futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>;

/// Direct Chrome DevTools Protocol page client via WebSocket.
///
/// Used when `AUTOCLI_CDP_ENDPOINT` is set (e.g., connecting to a headless Chrome instance).
pub struct CdpPage {
    ws_write: Mutex<WsSink>,
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<Value>>>>,
    cmd_id: AtomicU64,
}

impl CdpPage {
    /// Connect to a CDP WebSocket endpoint (e.g., `ws://127.0.0.1:9222/devtools/page/...`).
    pub async fn connect(endpoint: &str) -> Result<Self, CliError> {
        let (ws_stream, _) = connect_async(endpoint).await.map_err(|e| {
            CliError::browser_connect(format!("Failed to connect to CDP endpoint: {e}"))
        })?;

        let (write, mut read) = ws_stream.split();
        let pending: Arc<RwLock<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Spawn reader task
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            if let Some(id) = json.get("id").and_then(|v| v.as_u64()) {
                                if let Some(tx) =
                                    reader_pending.write().await.remove(&id)
                                {
                                    let _ = tx.send(json);
                                }
                            } else {
                                debug!(event = %text.chars().take(100).collect::<String>(), "CDP event");
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        debug!("CDP WebSocket closed");
                        break;
                    }
                    Err(e) => {
                        error!("CDP WebSocket read error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            ws_write: Mutex::new(write),
            pending,
            cmd_id: AtomicU64::new(1),
        })
    }

    /// Send a CDP command and await the response.
    async fn send_cdp_raw(&self, method: &str, params: Value) -> Result<Value, CliError> {
        let id = self.cmd_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        self.pending.write().await.insert(id, tx);

        {
            let mut ws = self.ws_write.lock().await;
            ws.send(Message::Text(msg.to_string().into()))
                .await
                .map_err(|e| CliError::browser_connect(format!("CDP send error: {e}")))?;
        }

        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(result)) => {
                if let Some(err) = result.get("error") {
                    Err(CliError::command_execution(format!(
                        "CDP error: {}",
                        err.get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown")
                    )))
                } else {
                    Ok(result.get("result").cloned().unwrap_or(Value::Null))
                }
            }
            Ok(Err(_)) => Err(CliError::browser_connect("CDP response channel closed")),
            Err(_) => {
                self.pending.write().await.remove(&id);
                Err(CliError::timeout("CDP command timed out (60s)"))
            }
        }
    }

    /// Evaluate JS via Runtime.evaluate.
    async fn evaluate_js(&self, expression: &str, await_promise: bool) -> Result<Value, CliError> {
        let result = self
            .send_cdp_raw(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": await_promise,
                }),
            )
            .await?;

        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("JS exception");
            return Err(CliError::command_execution(format!("JS error: {text}")));
        }

        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }
}

#[async_trait]
impl IPage for CdpPage {
    async fn goto(&self, url: &str, _options: Option<GotoOptions>) -> Result<(), CliError> {
        self.send_cdp_raw("Page.navigate", json!({ "url": url }))
            .await?;
        // Wait for load event
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }

    async fn url(&self) -> Result<String, CliError> {
        let val = self.evaluate_js("window.location.href", false).await?;
        Ok(val.as_str().unwrap_or("").to_string())
    }

    async fn title(&self) -> Result<String, CliError> {
        let val = self.evaluate_js("document.title", false).await?;
        Ok(val.as_str().unwrap_or("").to_string())
    }

    async fn content(&self) -> Result<String, CliError> {
        let val = self
            .evaluate_js("document.documentElement.outerHTML", false)
            .await?;
        Ok(val.as_str().unwrap_or("").to_string())
    }

    async fn evaluate(&self, expression: &str) -> Result<Value, CliError> {
        // Auto-detect async by checking for common async patterns
        let is_async = expression.contains("await ") || expression.starts_with("(async");
        self.evaluate_js(expression, is_async).await
    }

    async fn wait_for_selector(
        &self,
        selector: &str,
        options: Option<WaitOptions>,
    ) -> Result<(), CliError> {
        let opts = options.unwrap_or_default();
        let timeout = opts.timeout_ms.unwrap_or(30_000);
        let visible = opts.visible.unwrap_or(false);
        let js = dom_helpers::wait_for_selector_js(selector, timeout, visible);
        self.evaluate_js(&js, true).await?;
        Ok(())
    }

    async fn wait_for_navigation(&self, _options: Option<WaitOptions>) -> Result<(), CliError> {
        let js = dom_helpers::wait_for_dom_stable_js();
        self.evaluate_js(&js, true).await?;
        Ok(())
    }

    async fn wait_for_timeout(&self, ms: u64) -> Result<(), CliError> {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(())
    }

    async fn click(&self, selector: &str) -> Result<(), CliError> {
        let js = dom_helpers::click_js(selector);
        self.evaluate_js(&js, false).await?;
        Ok(())
    }

    async fn type_text(&self, selector: &str, text: &str) -> Result<(), CliError> {
        let js = dom_helpers::type_text_js(selector, text);
        self.evaluate_js(&js, false).await?;
        Ok(())
    }

    async fn cookies(&self, _options: Option<CookieOptions>) -> Result<Vec<Cookie>, CliError> {
        let result = self
            .send_cdp_raw("Network.getCookies", json!({}))
            .await?;
        let cookies_val = result.get("cookies").cloned().unwrap_or(json!([]));
        let cookies: Vec<Cookie> = serde_json::from_value(cookies_val).unwrap_or_default();
        Ok(cookies)
    }

    async fn set_cookies(&self, cookies: Vec<Cookie>) -> Result<(), CliError> {
        for cookie in &cookies {
            self.send_cdp_raw(
                "Network.setCookie",
                json!({
                    "name": cookie.name,
                    "value": cookie.value,
                    "domain": cookie.domain,
                    "path": cookie.path.as_deref().unwrap_or("/"),
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn screenshot(&self, _options: Option<ScreenshotOptions>) -> Result<Vec<u8>, CliError> {
        let result = self
            .send_cdp_raw("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        if let Some(data) = result.get("data").and_then(|d| d.as_str()) {
            Ok(crate::page::base64_decode_simple(data))
        } else {
            Ok(Vec::new())
        }
    }

    async fn snapshot(&self, options: Option<SnapshotOptions>) -> Result<Value, CliError> {
        let opts = options.unwrap_or_default();
        let js = dom_helpers::snapshot_js(opts.selector.as_deref(), opts.include_hidden);
        self.evaluate_js(&js, false).await
    }

    async fn auto_scroll(&self, options: Option<AutoScrollOptions>) -> Result<(), CliError> {
        let opts = options.unwrap_or_default();
        let max = opts.max_scrolls.unwrap_or(20);
        let delay = opts.delay_ms.unwrap_or(300);
        let js = dom_helpers::auto_scroll_js(max, delay);
        self.evaluate_js(&js, true).await?;
        Ok(())
    }

    async fn tabs(&self) -> Result<Vec<TabInfo>, CliError> {
        let result = self.send_cdp_raw("Target.getTargets", json!({})).await?;
        let targets = result
            .get("targetInfos")
            .cloned()
            .unwrap_or(json!([]));
        let mut tabs = Vec::new();
        if let Some(arr) = targets.as_array() {
            for t in arr {
                if t.get("type").and_then(|v| v.as_str()) == Some("page") {
                    tabs.push(TabInfo {
                        id: t
                            .get("targetId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        url: t
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        title: t
                            .get("title")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    });
                }
            }
        }
        Ok(tabs)
    }

    async fn switch_tab(&self, tab_id: &str) -> Result<(), CliError> {
        self.send_cdp_raw("Target.activateTarget", json!({ "targetId": tab_id }))
            .await?;
        Ok(())
    }

    async fn close(&self) -> Result<(), CliError> {
        self.send_cdp_raw("Browser.close", json!({})).await.ok();
        Ok(())
    }

    async fn intercept_requests(&self, url_pattern: &str) -> Result<(), CliError> {
        let js = dom_helpers::install_interceptor_js(url_pattern);
        self.evaluate_js(&js, false).await?;
        Ok(())
    }

    async fn get_intercepted_requests(&self) -> Result<Vec<InterceptedRequest>, CliError> {
        let js = dom_helpers::get_intercepted_requests_js();
        let val = self.evaluate_js(&js, false).await?;
        let reqs: Vec<InterceptedRequest> = serde_json::from_value(val).unwrap_or_default();
        Ok(reqs)
    }

    async fn get_network_requests(&self) -> Result<Vec<NetworkRequest>, CliError> {
        let js = dom_helpers::network_requests_js();
        let val = self.evaluate_js(&js, false).await?;
        let reqs: Vec<NetworkRequest> = serde_json::from_value(val).unwrap_or_default();
        Ok(reqs)
    }

    /// hermesDr fork · expose raw CDP send to YAML pipeline (for Input.insertText / Input.dispatchKeyEvent).
    async fn send_cdp(&self, method: &str, params: Value) -> Result<Value, CliError> {
        self.send_cdp_raw(method, params).await
    }

    /// hermesDr fork (S321 gh#34) · programmatic file upload via direct CDP.
    ///
    /// Uses Puppeteer's standard pattern: Runtime.evaluate with
    /// `returnByValue:false` to get the input element as a RemoteObject (with
    /// an objectId), then DOM.setFileInputFiles({objectId, files}). Empirically
    /// Chrome 147 silently no-op's setFileInputFiles when called with `nodeId`
    /// (from DOM.getDocument + DOM.querySelector chain) over a chrome.debugger
    /// or direct-CDP attach — but the **objectId** path is the one Puppeteer
    /// and Playwright use in production and is not subject to that silent
    /// rejection.
    async fn set_file_input(
        &self,
        selector: &str,
        files: Vec<String>,
    ) -> Result<(), CliError> {
        let query = if selector.is_empty() {
            "input[type=\"file\"]"
        } else {
            selector
        };
        let query_lit = serde_json::to_string(query)
            .unwrap_or_else(|_| "\"input[type=\\\"file\\\"]\"".into());

        // Enable domains Puppeteer typically warms up before DOM manipulation.
        // Chrome may silently restrict setFileInputFiles otherwise.
        let _ = self.send_cdp_raw("Page.enable", json!({})).await;
        let _ = self.send_cdp_raw("Runtime.enable", json!({})).await;
        let _ = self.send_cdp_raw("DOM.enable", json!({})).await;

        // Topview's input has class="hidden" (display:none). Some Chrome versions
        // silently no-op setFileInputFiles on inputs that aren't laid out. Force
        // it visible (1x1 fixed-pos, near-transparent) before the call; restore after.
        let unhide_expr = format!(
            r#"(() => {{
                const inp = document.querySelector({q});
                if (!inp) return null;
                inp.__s321_origStyle = inp.getAttribute('style') || '';
                inp.__s321_origClass = inp.className;
                inp.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0.001;display:block;visibility:visible;pointer-events:auto;z-index:2147483647';
                if (typeof inp.className === 'string' && inp.className.indexOf('hidden') >= 0) {{
                    inp.className = inp.className.replace(/\bhidden\b/g, ' ').trim();
                }}
                inp.removeAttribute('hidden');
                return inp;
            }})()"#,
            q = query_lit
        );
        let eval = self
            .send_cdp_raw(
                "Runtime.evaluate",
                json!({
                    "expression": unhide_expr,
                    "returnByValue": false,
                    "awaitPromise": false,
                }),
            )
            .await?;
        if let Some(exc) = eval.get("exceptionDetails") {
            return Err(CliError::command_execution(format!(
                "set_file_input: unhide/resolve eval threw: {}",
                exc.get("text").and_then(|t| t.as_str()).unwrap_or("?")
            )));
        }
        let result_obj = eval
            .get("result")
            .ok_or_else(|| CliError::command_execution("Runtime.evaluate returned no result"))?;
        let subtype = result_obj.get("subtype").and_then(|s| s.as_str());
        let object_id = result_obj
            .get("objectId")
            .and_then(|o| o.as_str())
            .ok_or_else(|| {
                if subtype == Some("null") {
                    CliError::command_execution(format!(
                        "set_file_input: no element matched selector {}",
                        query
                    ))
                } else {
                    CliError::command_execution(format!(
                        "set_file_input: Runtime.evaluate returned no objectId (subtype={:?})",
                        subtype
                    ))
                }
            })?
            .to_string();

        let set_response = self
            .send_cdp_raw(
                "DOM.setFileInputFiles",
                json!({"objectId": object_id, "files": files}),
            )
            .await?;
        tracing::info!(?set_response, "DOM.setFileInputFiles raw response");

        // Read back .files.length immediately so we can store diag for yaml to inspect.
        // Also restore original style/class.
        let verify_expr = format!(
            r#"(() => {{
                const inp = document.querySelector({q});
                const out = {{ filesLen: inp ? inp.files.length : -1,
                               firstName: inp && inp.files[0] ? inp.files[0].name : null,
                               firstSize: inp && inp.files[0] ? inp.files[0].size : null,
                               isConnected: inp ? inp.isConnected : null,
                               className: inp ? (inp.className || '') : null }};
                if (inp) {{
                    inp.setAttribute('style', inp.__s321_origStyle || '');
                    if (typeof inp.__s321_origClass === 'string') inp.className = inp.__s321_origClass;
                    delete inp.__s321_origStyle; delete inp.__s321_origClass;
                }}
                window.__upDiagCdp = out;
                return out;
            }})()"#,
            q = query_lit
        );
        let verify = self
            .send_cdp_raw(
                "Runtime.evaluate",
                json!({"expression": verify_expr, "returnByValue": true}),
            )
            .await?;
        tracing::info!(?verify, "post-setFileInputFiles verify result");

        let _ = self
            .send_cdp_raw("Runtime.releaseObject", json!({"objectId": object_id}))
            .await;

        Ok(())
    }
}
