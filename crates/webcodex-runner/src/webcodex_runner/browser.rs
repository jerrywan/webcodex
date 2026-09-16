use super::{ok_cmd, CommandResult};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use std::time::Instant;
use webcodex_browser::{
    BrowserError, BrowserKey, BrowserResult, BrowserSupervisor, ExecutionState, MAX_PAGE_SUMMARIES,
};
use webcodex_core::runner_operation::{RunnerBrowserOperation, RunnerBrowserOperationKind};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserRequest {
    browser_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PagesRequest {
    browser_id: String,
    #[serde(default = "default_page_limit")]
    limit: usize,
}

fn default_page_limit() -> usize {
    MAX_PAGE_SUMMARIES
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageRequest {
    browser_id: String,
    page_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateRequest {
    browser_id: String,
    page_id: String,
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ElementRequest {
    browser_id: String,
    page_id: String,
    element_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputTextRequest {
    browser_id: String,
    page_id: String,
    element_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRequest {
    browser_id: String,
    page_id: String,
    key: BrowserKey,
}

pub(crate) fn handle_browser_operation(
    supervisor: &BrowserSupervisor,
    operation: &RunnerBrowserOperation,
) -> CommandResult {
    let start = Instant::now();
    let output =
        match operation.kind {
            RunnerBrowserOperationKind::ListBrowsers => parse::<EmptyRequest>(&operation.payload)
                .map(|_| {
                    let browsers = supervisor.list_browsers();
                    json!({
                        "count": browsers.len(),
                        "browsers": browsers,
                    })
                }),
            RunnerBrowserOperationKind::ListPages => parse::<PagesRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .pages(&request.browser_id, request.limit)
                        .map(|pages| {
                            json!({
                                "count": pages.len(),
                                "pages": pages,
                            })
                        })
                }),
            RunnerBrowserOperationKind::Snapshot => parse::<PageRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .snapshot(&request.browser_id, &request.page_id)
                        .map(|v| json!(v))
                }),
            RunnerBrowserOperationKind::Screenshot => parse::<PageRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .screenshot(&request.browser_id, &request.page_id)
                        .map(|v| json!(v))
                }),
            RunnerBrowserOperationKind::Launch => parse::<EmptyRequest>(&operation.payload)
                .and_then(|_| supervisor.launch().map(|v| json!(v))),
            RunnerBrowserOperationKind::NewPage => parse::<BrowserRequest>(&operation.payload)
                .and_then(|request| supervisor.new_page(&request.browser_id).map(|v| json!(v))),
            RunnerBrowserOperationKind::Navigate => parse::<NavigateRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .navigate(&request.browser_id, &request.page_id, &request.url)
                        .map(|_| json!({}))
                }),
            RunnerBrowserOperationKind::Click => parse::<ElementRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .click(&request.browser_id, &request.page_id, &request.element_id)
                        .map(|_| json!({}))
                }),
            RunnerBrowserOperationKind::InputText => parse::<InputTextRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .input_text(
                            &request.browser_id,
                            &request.page_id,
                            &request.element_id,
                            &request.text,
                        )
                        .map(|_| json!({}))
                }),
            RunnerBrowserOperationKind::Key => {
                parse::<KeyRequest>(&operation.payload).and_then(|request| {
                    supervisor
                        .key(&request.browser_id, &request.page_id, request.key)
                        .map(|_| json!({}))
                })
            }
            RunnerBrowserOperationKind::ClosePage => parse::<PageRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .close_page(&request.browser_id, &request.page_id)
                        .map(|_| json!({}))
                }),
            RunnerBrowserOperationKind::CloseBrowser => parse::<BrowserRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .close_browser(&request.browser_id)
                        .map(|_| json!({}))
                }),
        };

    let value = match output {
        Ok(result) => json!({
            "ok": true,
            "execution_state": ExecutionState::Completed,
            "result": result,
        }),
        Err(error) => json!({
            "ok": false,
            "execution_state": error.execution_state,
            "error": error,
        }),
    };
    ok_cmd(start, value)
}

fn parse<T: DeserializeOwned>(payload: &str) -> BrowserResult<T> {
    serde_json::from_str(payload).map_err(|error| {
        BrowserError::not_started(
            "invalid_request",
            format!("Browser operation payload is invalid: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use webcodex_core::runner_operation::RunnerBrowserOperationKind;

    #[test]
    fn unrelated_fields_fail_closed_before_effect() {
        let operation = RunnerBrowserOperation {
            kind: RunnerBrowserOperationKind::Launch,
            payload: r#"{"executable":"/tmp/chrome"}"#.to_string(),
            timeout_secs: 30,
        };
        let result = handle_browser_operation(&BrowserSupervisor::new(), &operation);
        let output: Value = serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(output["ok"], false);
        assert_eq!(output["execution_state"], "not_started");
        assert_eq!(output["error"]["kind"], "invalid_request");
    }
}
