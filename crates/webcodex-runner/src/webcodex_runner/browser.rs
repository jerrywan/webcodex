use super::config::RunnerPolicy;
use super::shell::cwd_allowed;
use super::{ok_cmd, CommandResult};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
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
struct SelectOptionRequest {
    browser_id: String,
    page_id: String,
    element_id: String,
    option: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetValueRequest {
    browser_id: String,
    page_id: String,
    element_id: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadFileRequest {
    browser_id: String,
    page_id: String,
    element_id: String,
    project_root: String,
    path: String,
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
    policy: &RunnerPolicy,
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
            RunnerBrowserOperationKind::SelectOption => {
                parse::<SelectOptionRequest>(&operation.payload).and_then(|request| {
                    supervisor
                        .select_option(
                            &request.browser_id,
                            &request.page_id,
                            &request.element_id,
                            &request.option,
                        )
                        .map(|_| json!({}))
                })
            }
            RunnerBrowserOperationKind::SetValue => parse::<SetValueRequest>(&operation.payload)
                .and_then(|request| {
                    supervisor
                        .set_value(
                            &request.browser_id,
                            &request.page_id,
                            &request.element_id,
                            &request.value,
                        )
                        .map(|_| json!({}))
                }),
            RunnerBrowserOperationKind::UploadFile => {
                parse::<UploadFileRequest>(&operation.payload).and_then(|request| {
                    let path = resolve_upload_path(policy, &request.project_root, &request.path)?;
                    supervisor
                        .upload_file(
                            &request.browser_id,
                            &request.page_id,
                            &request.element_id,
                            &path,
                        )
                        .map(|_| json!({}))
                })
            }
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

const MAX_BROWSER_UPLOAD_FILE_BYTES: u64 = 32 * 1024 * 1024;

fn resolve_upload_path(
    policy: &RunnerPolicy,
    project_root: &str,
    relative_path: &str,
) -> BrowserResult<PathBuf> {
    if project_root.is_empty()
        || project_root.contains('\0')
        || relative_path.is_empty()
        || relative_path.contains('\0')
    {
        return Err(BrowserError::not_started(
            "invalid_upload_path",
            "Browser upload requires a non-empty project root and project-relative file path",
        ));
    }
    let raw_root = Path::new(project_root);
    let raw_relative = Path::new(relative_path);
    if !raw_root.is_absolute()
        || raw_relative.is_absolute()
        || raw_relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BrowserError::not_started(
            "invalid_upload_path",
            "Browser upload path must be a normal project-relative path",
        ));
    }
    let root = raw_root.canonicalize().map_err(|_| {
        BrowserError::not_started(
            "upload_project_unavailable",
            "Browser upload project root is unavailable",
        )
    })?;
    if !root.is_dir() || cwd_allowed(policy, &root).is_err() {
        return Err(BrowserError::not_started(
            "upload_project_denied",
            "Browser upload project root is outside Runner path policy",
        ));
    }
    let path = root.join(raw_relative).canonicalize().map_err(|_| {
        BrowserError::not_started(
            "upload_file_unavailable",
            "Browser upload file is unavailable",
        )
    })?;
    if !webcodex_runner_config::paths::path_is_within(&path, &root)
        || cwd_allowed(policy, &path).is_err()
    {
        return Err(BrowserError::not_started(
            "upload_path_escape",
            "Browser upload file resolves outside the authorized project root",
        ));
    }
    let canonical_relative = path.strip_prefix(&root).map_err(|_| {
        BrowserError::not_started(
            "upload_path_escape",
            "Browser upload file resolves outside the authorized project root",
        )
    })?;
    if webcodex_core::sensitive_paths::is_secret_path(relative_path)
        || webcodex_core::sensitive_paths::is_secret_path(
            canonical_relative.to_string_lossy().as_ref(),
        )
    {
        return Err(BrowserError::not_started(
            "upload_sensitive_path",
            "Browser upload refuses sensitive project paths",
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|_| {
        BrowserError::not_started(
            "upload_file_unavailable",
            "Browser upload file is unavailable",
        )
    })?;
    if !metadata.is_file() {
        return Err(BrowserError::not_started(
            "upload_file_invalid",
            "Browser upload source must be a regular file",
        ));
    }
    if metadata.len() > MAX_BROWSER_UPLOAD_FILE_BYTES {
        return Err(BrowserError::not_started(
            "upload_file_too_large",
            "Browser upload file exceeds the 32 MiB bound",
        ));
    }
    Ok(path)
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
        let result = handle_browser_operation(
            &BrowserSupervisor::new(),
            &RunnerPolicy::default(),
            &operation,
        );
        let output: Value = serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(output["ok"], false);
        assert_eq!(output["execution_state"], "not_started");
        assert_eq!(output["error"]["kind"], "invalid_request");
    }

    #[test]
    fn upload_path_is_project_relative_bounded_and_regular() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let resume = root.join("resume.pdf");
        std::fs::write(&resume, b"fixture").unwrap();

        let mut policy = RunnerPolicy::default();
        policy.allowed_roots = vec![root.clone()];

        let resolved =
            resolve_upload_path(&policy, root.to_string_lossy().as_ref(), "resume.pdf").unwrap();
        assert_eq!(resolved, resume.canonicalize().unwrap());

        std::fs::write(root.join(".env"), b"SECRET=value").unwrap();
        let sensitive =
            resolve_upload_path(&policy, root.to_string_lossy().as_ref(), ".env").unwrap_err();
        assert_eq!(sensitive.kind, "upload_sensitive_path");

        for denied in ["", "..\\outside.pdf", "../outside.pdf", "."] {
            assert!(resolve_upload_path(&policy, root.to_string_lossy().as_ref(), denied).is_err());
        }

        let missing =
            resolve_upload_path(&policy, root.to_string_lossy().as_ref(), "subdir").unwrap_err();
        assert_eq!(missing.kind, "upload_file_unavailable");

        std::fs::create_dir_all(root.join("subdir")).unwrap();
        let directory =
            resolve_upload_path(&policy, root.to_string_lossy().as_ref(), "subdir").unwrap_err();
        assert_eq!(directory.kind, "upload_file_invalid");

        let oversized_path = root.join("oversized.pdf");
        let file = std::fs::File::create(&oversized_path).unwrap();
        file.set_len(MAX_BROWSER_UPLOAD_FILE_BYTES + 1).unwrap();
        let oversized =
            resolve_upload_path(&policy, root.to_string_lossy().as_ref(), "oversized.pdf")
                .unwrap_err();
        assert_eq!(oversized.kind, "upload_file_too_large");
    }
}
