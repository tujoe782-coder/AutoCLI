use autocli_core::{CliError, IPage};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::step_registry::StepRegistry;

const MAX_BROWSER_ATTEMPTS: usize = 3;

/// Execute a pipeline — a sequence of steps.
///
/// Each step is a YAML object like `{ "fetch": "https://..." }` or
/// `{ "map": { "title": "${{ item.title }}" } }`.
///
/// Steps are executed sequentially. Each step receives the current `data` and
/// returns new `data`. Browser steps get up to 2 retries on transient errors.
///
/// **S322 saveTo extension**: A step may optionally include a `saveTo` sibling
/// key (peer to the step name). When set, the step's return value is wrapped
/// into `data[key]` (namespacing) instead of replacing `data` entirely. This
/// enables multi-step pipelines that need to combine outputs from several
/// steps (e.g. fetch STS credentials → use them in a later upload step).
///
/// Yaml example:
/// ```yaml
/// - fetch:
///     url: "https://example.com"
///   saveTo: response   # New: namespaces result under data.response
/// - tap:
///     message: "got ${{ data.response.body }}"
/// ```
///
/// Backward compat: steps without `saveTo` keep legacy "replace data" behavior.
pub async fn execute_pipeline(
    page: Option<Arc<dyn IPage>>,
    pipeline: &[Value],
    args: &HashMap<String, Value>,
    registry: &StepRegistry,
) -> Result<Value, CliError> {
    let mut data = Value::Null;

    for (i, step) in pipeline.iter().enumerate() {
        let obj = step.as_object().ok_or_else(|| {
            CliError::pipeline(format!("Step {i} is not an object: {step}"))
        })?;

        // S322: extract optional `saveTo` key (top-level, peer to step name).
        let save_to: Option<String> = obj
            .get("saveTo")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Step name + params = the single non-saveTo entry.
        let step_entries: Vec<(&String, &Value)> = obj
            .iter()
            .filter(|(k, _)| k.as_str() != "saveTo")
            .collect();

        if step_entries.len() != 1 {
            let keys: Vec<&str> = step_entries.iter().map(|(k, _)| k.as_str()).collect();
            return Err(CliError::pipeline(format!(
                "Step {i} must have exactly one step key (plus optional saveTo), found {} non-saveTo keys: {:?}",
                step_entries.len(),
                keys
            )));
        }

        let (step_name, params) = step_entries[0];

        let handler = registry.get(step_name).ok_or_else(|| {
            CliError::pipeline(format!("Unknown step '{step_name}' at index {i}"))
        })?;

        let is_browser = handler.is_browser_step();
        let mut last_error: Option<CliError> = None;

        for attempt in 0..if is_browser { MAX_BROWSER_ATTEMPTS } else { 1 } {
            match handler
                .execute(page.clone(), params, &data, args)
                .await
            {
                Ok(result) => {
                    match &save_to {
                        Some(key) => {
                            // S322: wrap result into data[key] (namespace).
                            // If data is already an Object, extend it; otherwise
                            // start a fresh Object containing just this key.
                            let prev = std::mem::replace(&mut data, Value::Null);
                            let mut map = match prev {
                                Value::Object(m) => m,
                                _ => serde_json::Map::new(),
                            };
                            map.insert(key.clone(), result);
                            data = Value::Object(map);
                        }
                        None => {
                            // Legacy: replace data entirely.
                            data = result;
                        }
                    }
                    last_error = None;
                    break;
                }
                Err(e) => {
                    if is_browser && attempt + 1 < MAX_BROWSER_ATTEMPTS {
                        warn!(
                            step = step_name,
                            attempt = attempt + 1,
                            "Browser step failed, retrying: {e}"
                        );
                        last_error = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        if let Some(e) = last_error {
            return Err(e);
        }
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step_registry::StepHandler;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EchoStep;

    #[async_trait]
    impl StepHandler for EchoStep {
        fn name(&self) -> &'static str {
            "echo"
        }

        async fn execute(
            &self,
            _page: Option<Arc<dyn IPage>>,
            params: &Value,
            _data: &Value,
            _args: &HashMap<String, Value>,
        ) -> Result<Value, CliError> {
            Ok(params.clone())
        }
    }

    /// A step that appends its params to the current data array, or wraps data
    /// in an array if it is not already one.
    struct AppendStep;

    #[async_trait]
    impl StepHandler for AppendStep {
        fn name(&self) -> &'static str {
            "append"
        }

        async fn execute(
            &self,
            _page: Option<Arc<dyn IPage>>,
            params: &Value,
            data: &Value,
            _args: &HashMap<String, Value>,
        ) -> Result<Value, CliError> {
            let mut arr = match data {
                Value::Array(a) => a.clone(),
                Value::Null => vec![],
                other => vec![other.clone()],
            };
            arr.push(params.clone());
            Ok(Value::Array(arr))
        }
    }

    /// A browser step that fails the first N times, then succeeds.
    struct FlakyBrowserStep {
        fail_count: AtomicUsize,
        fail_times: usize,
    }

    impl FlakyBrowserStep {
        fn new(fail_times: usize) -> Self {
            Self {
                fail_count: AtomicUsize::new(0),
                fail_times,
            }
        }
    }

    #[async_trait]
    impl StepHandler for FlakyBrowserStep {
        fn name(&self) -> &'static str {
            "flaky_browser"
        }

        fn is_browser_step(&self) -> bool {
            true
        }

        async fn execute(
            &self,
            _page: Option<Arc<dyn IPage>>,
            params: &Value,
            _data: &Value,
            _args: &HashMap<String, Value>,
        ) -> Result<Value, CliError> {
            let count = self.fail_count.fetch_add(1, Ordering::SeqCst);
            if count < self.fail_times {
                Err(CliError::pipeline("transient browser error"))
            } else {
                Ok(params.clone())
            }
        }
    }

    fn empty_args() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[tokio::test]
    async fn empty_pipeline_returns_null() {
        let registry = StepRegistry::new();
        let result = execute_pipeline(None, &[], &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn single_step_returns_step_output() {
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(EchoStep));

        let pipeline = vec![json!({"echo": "hello"})];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!("hello"));
    }

    #[tokio::test]
    async fn multi_step_pipeline_chains_data() {
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(AppendStep));

        let pipeline = vec![
            json!({"append": "first"}),
            json!({"append": "second"}),
            json!({"append": "third"}),
        ];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!(["first", "second", "third"]));
    }

    #[tokio::test]
    async fn unknown_step_returns_error() {
        let registry = StepRegistry::new();
        let pipeline = vec![json!({"nonexistent": null})];
        let err = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nonexistent"), "Error should mention the step name: {msg}");
    }

    #[tokio::test]
    async fn browser_step_retries_on_transient_error() {
        let mut registry = StepRegistry::new();
        // Fails twice then succeeds — should succeed on 3rd attempt
        registry.register(Arc::new(FlakyBrowserStep::new(2)));

        let pipeline = vec![json!({"flaky_browser": "ok"})];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!("ok"));
    }

    #[tokio::test]
    async fn browser_step_fails_after_max_retries() {
        let mut registry = StepRegistry::new();
        // Fails 3 times — all 3 attempts exhausted
        registry.register(Arc::new(FlakyBrowserStep::new(3)));

        let pipeline = vec![json!({"flaky_browser": "ok"})];
        let err = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("transient browser error"));
    }

    // ─── S322: saveTo namespacing tests ──────────────────────────────────

    #[tokio::test]
    async fn save_to_namespaces_result_under_key() {
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(EchoStep));

        let pipeline = vec![
            json!({"echo": "hello", "saveTo": "greeting"}),
            json!({"echo": "world", "saveTo": "subject"}),
        ];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!({"greeting": "hello", "subject": "world"}));
    }

    #[tokio::test]
    async fn save_to_extends_existing_namespace() {
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(EchoStep));

        let pipeline = vec![
            json!({"echo": {"key1": "val1"}, "saveTo": "first"}),
            json!({"echo": {"key2": "val2"}, "saveTo": "second"}),
        ];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!({
            "first": {"key1": "val1"},
            "second": {"key2": "val2"}
        }));
    }

    #[tokio::test]
    async fn step_without_save_to_replaces_data_backward_compat() {
        // Legacy yaml without saveTo must behave as before (replace).
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(EchoStep));

        let pipeline = vec![
            json!({"echo": "first"}),
            json!({"echo": "second"}),
        ];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!("second"));
    }

    #[tokio::test]
    async fn save_to_after_non_save_to_starts_fresh_namespace() {
        // If a prior step left data as non-Object, saveTo starts fresh.
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(EchoStep));

        let pipeline = vec![
            json!({"echo": "ignored"}),  // data becomes String "ignored"
            json!({"echo": "kept", "saveTo": "result"}),
        ];
        let result = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap();
        assert_eq!(result, json!({"result": "kept"}));
    }

    #[tokio::test]
    async fn save_to_rejects_two_non_save_to_keys() {
        // {echo: ..., append: ..., saveTo: ...} is invalid — must have exactly
        // one step key besides saveTo.
        let mut registry = StepRegistry::new();
        registry.register(Arc::new(EchoStep));
        registry.register(Arc::new(AppendStep));

        let pipeline = vec![
            json!({"echo": "a", "append": "b", "saveTo": "x"}),
        ];
        let err = execute_pipeline(None, &pipeline, &empty_args(), &registry)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exactly one step key"),
            "expected error to mention 'exactly one step key', got: {}", err);
    }
}
