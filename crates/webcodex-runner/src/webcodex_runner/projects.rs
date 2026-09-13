use super::config::{default_true, RunnerPolicy};
use super::shell::canonicalize_existing;
use crate::runner_protocol::RunnerProjectSummary;
#[cfg(test)]
use crate::runner_protocol::RunnerRequest;
use crate::{err_cmd, ok_cmd, write_created_file};
use crate::{CommandResult, CreatedProjectPaths};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(test)]
use webcodex_core::runner_operation::RunnerOperation;
use webcodex_core::runner_operation::{RunnerProjectOperation, RunnerProjectOperationKind};
use webcodex_runner_config::paths::paths_equal;

pub(crate) mod catalog;
pub(crate) mod registration;

#[cfg(test)]
pub(crate) use catalog::runner_project_summary;
use catalog::{
    effective_registration_source, project_lineage, project_revision, project_wire_kind,
    run_git_bounded, AUTO_REGISTERED_REGISTRATION_SOURCE, EXPLICIT_REGISTRATION_SOURCE,
};
pub(crate) use catalog::{
    find_project_shell_context, find_project_shell_context_by_id,
    load_runner_project_summaries_from_dir, parse_runner_project_toml, project_root_fingerprint,
};

use registration::{
    bounded_project_name, build_project_toml, build_project_toml_with_registration_source,
    choose_auto_project_id, load_project_files_for_path_resolution,
    projects_matching_canonical_path, resolve_managed_source_project, sanitized_project_basename,
    sync_dir, sync_parent_dir, sync_project_parent_after_rename, toml_basic_string,
    unique_registry_temp, validate_model_network_project_ingress_authority,
    validate_project_op_description, validate_project_op_id, validate_project_op_name,
    validate_windows_project_root, write_project_toml_atomic, ProjectTomlWriteError,
};
#[cfg(test)]
pub(crate) use registration::{
    fail_next_project_parent_sync_after_rename, fail_next_project_publish_before_rename,
};
pub(crate) use registration::{
    handle_resolve_or_register_project_operation, validate_project_path_policy,
};

const MANAGED_WORKTREE_GIT_TIMEOUT: Duration = Duration::from_secs(20);
static PROJECT_REGISTRY_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) fn project_registry_write_lock() -> &'static Mutex<()> {
    PROJECT_REGISTRY_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn project_error_cmd(start: Instant, error_code: &'static str) -> CommandResult {
    CommandResult {
        exit_code: Some(1),
        stdout: Some(
            serde_json::to_string(&serde_json::json!({"error_code": error_code}))
                .unwrap_or_else(|_| r#"{"error_code":"operation_failed"}"#.to_string()),
        ),
        stderr: Some(String::new()),
        duration_ms: Some(start.elapsed().as_millis() as u64),
        error: None,
    }
}

pub(super) fn structured_project_error_cmd(
    start: Instant,
    error_kind: &'static str,
    state_changed: bool,
    fields: serde_json::Value,
) -> CommandResult {
    let mut output = serde_json::json!({
        "error_code": error_kind,
        "error_kind": error_kind,
        "failure_kind": error_kind,
        "state_changed": state_changed,
    });
    if let (Some(output), Some(fields)) = (output.as_object_mut(), fields.as_object()) {
        output.extend(fields.clone());
    }
    CommandResult {
        exit_code: Some(1),
        stdout: Some(
            serde_json::to_string(&output)
                .unwrap_or_else(|_| r#"{"error_code":"operation_failed"}"#.to_string()),
        ),
        stderr: Some(String::new()),
        duration_ms: Some(start.elapsed().as_millis() as u64),
        error: None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RunnerProjectFile {
    pub(crate) id: String,
    pub(crate) path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shell_profile: Option<String>,
    #[serde(default = "default_true")]
    pub(crate) allow_patch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) registration_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) disabled: bool,
    #[serde(default)]
    pub(crate) hooks: HashMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) managed_worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_source_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_source_root_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_base_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_operation_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RunnerProjectCache {
    pub(super) projects: Vec<RunnerProjectSummary>,
    pub(super) refreshed_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub(crate) struct RunnerProjectShellContext {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) shell_profile: Option<String>,
}

#[derive(Debug)]
enum ProjectUnregisterError {
    BeforeRename,
    AfterRename,
}

fn managed_worktree_error(
    start: Instant,
    error_kind: &'static str,
    state_changed: bool,
    base_ref: Option<&str>,
    base_sha: Option<&str>,
    source_dirty: Option<bool>,
) -> CommandResult {
    structured_project_error_cmd(
        start,
        error_kind,
        state_changed,
        serde_json::json!({
            "base_ref": base_ref,
            "base_sha": base_sha,
            "source_dirty": source_dirty,
        }),
    )
}

fn managed_worktree_git_text(path: &Path, args: &[&str]) -> Result<String, &'static str> {
    let output = run_git_bounded(path, args, MANAGED_WORKTREE_GIT_TIMEOUT, None)
        .map_err(|_| "worktree_git_failed")?;
    if !output.status.success() || output.stdout_capped || output.stderr_capped {
        return Err("worktree_git_failed");
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| "worktree_git_failed")
}

fn exact_git_commit(source: &Path, base_ref: &str) -> Result<String, &'static str> {
    let commit_ref = format!("{base_ref}^{{commit}}");
    let output = run_git_bounded(
        source,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            commit_ref.as_str(),
        ],
        MANAGED_WORKTREE_GIT_TIMEOUT,
        None,
    )
    .map_err(|_| "base_ref_resolution_failed")?;
    if !output.status.success() || output.stdout_capped {
        return Err("base_ref_resolution_failed");
    }
    let sha = String::from_utf8(output.stdout)
        .map_err(|_| "base_ref_resolution_failed")?
        .trim()
        .to_string();
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("base_ref_resolution_failed");
    }
    Ok(sha.to_ascii_lowercase())
}

fn choose_managed_worktree_root(
    policy: &RunnerPolicy,
    source: &Path,
) -> Result<PathBuf, &'static str> {
    let mut roots =
        webcodex_runner_config::paths::canonicalize_usable_allowed_roots(&policy.allowed_roots);
    roots.sort_by_key(|root| std::cmp::Reverse(root.components().count()));
    for root in roots {
        if !webcodex_runner_config::paths::path_is_within(source, &root) {
            continue;
        }
        let candidate = root.join(".webcodex-managed-worktrees");
        if webcodex_runner_config::paths::path_is_within(&candidate, source) {
            continue;
        }
        return Ok(candidate);
    }
    if policy.allow_cwd_anywhere {
        if let Some(parent) = source.parent() {
            let candidate = parent.join(".webcodex-managed-worktrees");
            if !webcodex_runner_config::paths::path_is_within(&candidate, source) {
                return Ok(candidate);
            }
        }
    }
    Err("managed_worktree_root_unavailable")
}

/// Render an already-authorized managed-worktree destination for Git's CLI.
///
/// Rust canonicalization commonly returns `\\?\C:\...` on Windows. Win32
/// filesystem APIs accept that identity, but Git for Windows does not reliably
/// accept the verbatim-disk spelling as a `git worktree add` destination. Keep
/// canonical PathBuf values for policy, registry, recovery, and identity checks;
/// only the child-process argv gets the equivalent ordinary local-disk spelling.
fn managed_worktree_git_cli_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        let mut components = path.components();
        if let Some(Component::Prefix(prefix)) = components.next() {
            if let Prefix::VerbatimDisk(drive) = prefix.kind() {
                let mut rendered = PathBuf::from(format!("{}:\\", char::from(drive)));
                for component in components {
                    match component {
                        Component::RootDir => {}
                        Component::Normal(part) => rendered.push(part),
                        _ => return path.to_string_lossy().into_owned(),
                    }
                }
                return rendered.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string_lossy().into_owned()
}

fn source_mentions_worktree_path(source: &Path, worktree: &Path) -> Result<bool, &'static str> {
    let listing = managed_worktree_git_text(source, &["worktree", "list", "--porcelain"])?;
    Ok(listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|path| paths_equal(Path::new(path), worktree)))
}

fn managed_worktree_project_toml(
    id: &str,
    name: &str,
    worktree: &str,
    source: &str,
    source_project_id: &str,
    source_root_fingerprint: &str,
    base_ref: &str,
    base_sha: &str,
    operation_id: &str,
) -> String {
    let mut content = build_project_toml_with_registration_source(
        id,
        name,
        worktree,
        Some(AUTO_REGISTERED_REGISTRATION_SOURCE),
        &None,
        true,
    );
    content.push_str("managed_worktree = true\n");
    content.push_str(&format!("managed_source = {}\n", toml_basic_string(source)));
    content.push_str(&format!(
        "managed_source_project_id = {}\n",
        toml_basic_string(source_project_id)
    ));
    content.push_str(&format!(
        "managed_source_root_fingerprint = {}\n",
        toml_basic_string(source_root_fingerprint)
    ));
    content.push_str(&format!(
        "managed_base_ref = {}\n",
        toml_basic_string(base_ref)
    ));
    content.push_str(&format!(
        "managed_base_sha = {}\n",
        toml_basic_string(base_sha)
    ));
    content.push_str(&format!(
        "managed_operation_id = {}\n",
        toml_basic_string(operation_id)
    ));
    content
}

fn managed_worktree_success(
    start: Instant,
    client_id: &str,
    project: &RunnerProjectFile,
    worktree: &Path,
    base_ref: &str,
    base_sha: &str,
    source_dirty: bool,
    outcome: &'static str,
    registered: bool,
    changed: bool,
) -> CommandResult {
    ok_cmd(
        start,
        serde_json::json!({
            "id": format!("agent:{}:{}", client_id, project.id),
            "agent_project_id": project.id,
            "client_id": client_id,
            "name": project.name,
            "path": worktree.to_string_lossy(),
            "kind": project_wire_kind(project),
            "registration_source": effective_registration_source(project).as_str(),
            "description": project.description,
            "allow_patch": project.allow_patch,
            "disabled": project.disabled,
            "revision": project_revision(project),
            "root_fingerprint": project_root_fingerprint(worktree),
            "lineage": project_lineage(project),
            "source": "managed_worktree",
            "outcome": outcome,
            "registered": registered,
            "created_config": registered,
            "changed": changed,
            "recovered": outcome == "managed_worktree_recovered",
            "managed": true,
            "base_ref": base_ref,
            "base_sha": base_sha,
            "source_dirty": source_dirty,
        }),
    )
}

fn resume_managed_worktree(
    start: Instant,
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    client_id: &str,
    source_root: &Path,
    source_dirty: bool,
    requested_base_ref: Option<&str>,
    resume_project_id: &str,
) -> CommandResult {
    if validate_project_op_id(resume_project_id).is_err() {
        return managed_worktree_error(
            start,
            "invalid_request",
            false,
            requested_base_ref,
            None,
            Some(source_dirty),
        );
    }
    let projects = match load_project_files_for_path_resolution(project_registry_dir) {
        Ok(projects) => projects,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                false,
                requested_base_ref,
                None,
                Some(source_dirty),
            )
        }
    };
    let Some(project) = projects
        .iter()
        .find(|project| project.id == resume_project_id)
        .cloned()
    else {
        return managed_worktree_error(
            start,
            "managed_worktree_resume_mismatch",
            false,
            requested_base_ref,
            None,
            Some(source_dirty),
        );
    };
    let stored_base_sha = project.managed_base_sha.as_deref().filter(|sha| {
        matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let stored_source = project
        .managed_source
        .as_deref()
        .and_then(|path| canonicalize_existing(Path::new(path)).ok());
    if project.disabled
        || !project.managed_worktree
        || stored_base_sha.is_none()
        || stored_source
            .as_ref()
            .is_none_or(|source| !paths_equal(source, source_root))
    {
        return managed_worktree_error(
            start,
            "managed_worktree_resume_mismatch",
            false,
            requested_base_ref,
            stored_base_sha,
            Some(source_dirty),
        );
    }
    if let (Some(stored_source_project_id), Some(stored_source_root_fingerprint)) = (
        project.managed_source_project_id.as_deref(),
        project.managed_source_root_fingerprint.as_deref(),
    ) {
        let current_lineage = resolve_managed_source_project(&projects, source_root);
        if current_lineage.as_ref().is_err()
            || current_lineage
                .as_ref()
                .is_ok_and(|(source_project_id, source_root_fingerprint)| {
                    source_project_id != stored_source_project_id
                        || source_root_fingerprint != stored_source_root_fingerprint
                })
        {
            return managed_worktree_error(
                start,
                "managed_worktree_resume_mismatch",
                false,
                requested_base_ref,
                stored_base_sha,
                Some(source_dirty),
            );
        }
    }
    let base_sha = stored_base_sha.expect("validated managed base SHA");
    if let Some(base_ref) = requested_base_ref {
        match exact_git_commit(source_root, base_ref) {
            Ok(resolved) if resolved == base_sha => {}
            Ok(_) => {
                return managed_worktree_error(
                    start,
                    "managed_worktree_resume_base_mismatch",
                    false,
                    Some(base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
            Err(error) => {
                return managed_worktree_error(
                    start,
                    error,
                    false,
                    Some(base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        }
    }
    let worktree = match canonicalize_existing(Path::new(&project.path)) {
        Ok(worktree) => worktree,
        Err(_) => {
            return managed_worktree_error(
                start,
                "managed_worktree_recovery_conflict",
                false,
                project.managed_base_ref.as_deref(),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    if validate_windows_project_root(&worktree).is_err()
        || validate_project_path_policy(policy, &worktree).is_err()
    {
        return managed_worktree_error(
            start,
            "managed_worktree_recovery_conflict",
            false,
            project.managed_base_ref.as_deref(),
            Some(&base_sha),
            Some(source_dirty),
        );
    }
    let head = match managed_worktree_git_text(&worktree, &["rev-parse", "HEAD"]) {
        Ok(head) => head,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                false,
                project.managed_base_ref.as_deref(),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    let listed = match source_mentions_worktree_path(source_root, &worktree) {
        Ok(listed) => listed,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                false,
                project.managed_base_ref.as_deref(),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    if head != base_sha || !listed {
        return managed_worktree_error(
            start,
            "managed_worktree_recovery_conflict",
            false,
            project.managed_base_ref.as_deref(),
            Some(&base_sha),
            Some(source_dirty),
        );
    }
    let projected_base_ref = project
        .managed_base_ref
        .as_deref()
        .unwrap_or(requested_base_ref.unwrap_or("HEAD"));
    managed_worktree_success(
        start,
        client_id,
        &project,
        &worktree,
        projected_base_ref,
        &base_sha,
        source_dirty,
        "managed_worktree_recovered",
        false,
        false,
    )
}

/// Internal Server↔Runner operation that owns Git/ref/path semantics for managed
/// worktrees. It is intentionally not model-visible; successful output is fed
/// back through the ordinary registered runtime Project authority path.
pub(crate) fn handle_prepare_managed_worktree_operation(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    client_id: &str,
    operation: &RunnerProjectOperation,
) -> CommandResult {
    let start = Instant::now();
    let _registry_guard = match project_registry_write_lock().lock() {
        Ok(guard) => guard,
        Err(_) => {
            return managed_worktree_error(start, "operation_failed", false, None, None, None)
        }
    };
    let Some(payload) = serde_json::from_str::<serde_json::Value>(&operation.payload)
        .ok()
        .and_then(|payload| payload.as_object().cloned())
    else {
        return managed_worktree_error(start, "invalid_request", false, None, None, None);
    };
    if payload.len() != 4 {
        return managed_worktree_error(start, "invalid_request", false, None, None, None);
    }
    let Some(path) = payload
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty() && !path.contains('\0') && Path::new(path).is_absolute())
    else {
        return managed_worktree_error(start, "invalid_project_path", false, None, None, None);
    };
    let requested_base_ref = match payload.get("base_ref") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value))
            if !value.trim().is_empty() && value.len() <= 1024 && !value.contains('\0') =>
        {
            Some(value.clone())
        }
        _ => return managed_worktree_error(start, "invalid_base_ref", false, None, None, None),
    };
    let base_ref = requested_base_ref
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());
    let resume_project_id = match payload.get("resume_project_id") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) if !value.trim().is_empty() => Some(value.as_str()),
        _ => {
            return managed_worktree_error(
                start,
                "invalid_request",
                false,
                Some(&base_ref),
                None,
                None,
            )
        }
    };
    let Some(operation_id) = payload
        .get("operation_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
    else {
        return managed_worktree_error(
            start,
            "invalid_request",
            false,
            Some(&base_ref),
            None,
            None,
        );
    };
    if validate_windows_project_root(Path::new(path)).is_err() {
        return managed_worktree_error(
            start,
            "invalid_project_path",
            false,
            Some(&base_ref),
            None,
            None,
        );
    }
    if let Err(error_kind) =
        validate_model_network_project_ingress_authority(policy, Path::new(path))
    {
        return managed_worktree_error(start, error_kind, false, Some(&base_ref), None, None);
    }
    let source = match canonicalize_existing(Path::new(path)) {
        Ok(source) if source.is_dir() && source.to_str().is_some() => source,
        _ => {
            return managed_worktree_error(
                start,
                "project_path_not_found",
                false,
                Some(&base_ref),
                None,
                None,
            )
        }
    };
    if validate_windows_project_root(&source).is_err()
        || validate_project_path_policy(policy, &source).is_err()
    {
        return managed_worktree_error(
            start,
            "path_outside_allowed_roots",
            false,
            Some(&base_ref),
            None,
            None,
        );
    }
    let source_root = match managed_worktree_git_text(&source, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(|root| canonicalize_existing(Path::new(&root)).ok())
    {
        Some(root) if paths_equal(&root, &source) => root,
        _ => {
            return managed_worktree_error(
                start,
                "source_not_git_repository",
                false,
                Some(&base_ref),
                None,
                None,
            )
        }
    };
    let source_dirty = match managed_worktree_git_text(&source_root, &["status", "--porcelain"]) {
        Ok(status) => !status.is_empty(),
        Err(_) => {
            return managed_worktree_error(
                start,
                "source_git_status_failed",
                false,
                Some(&base_ref),
                None,
                None,
            )
        }
    };
    if let Some(resume_project_id) = resume_project_id {
        return resume_managed_worktree(
            start,
            policy,
            project_registry_dir,
            client_id,
            &source_root,
            source_dirty,
            requested_base_ref.as_deref(),
            resume_project_id,
        );
    }
    let source_projects = match load_project_files_for_path_resolution(project_registry_dir) {
        Ok(projects) => projects,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                false,
                Some(&base_ref),
                None,
                Some(source_dirty),
            )
        }
    };
    let (source_project_id, source_root_fingerprint) =
        match resolve_managed_source_project(&source_projects, &source_root) {
            Ok(lineage) => lineage,
            Err(error) => {
                return managed_worktree_error(
                    start,
                    error,
                    false,
                    Some(&base_ref),
                    None,
                    Some(source_dirty),
                )
            }
        };
    let base_sha = match exact_git_commit(&source_root, &base_ref) {
        Ok(sha) => sha,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                false,
                Some(&base_ref),
                None,
                Some(source_dirty),
            )
        }
    };
    let managed_root = match choose_managed_worktree_root(policy, &source_root) {
        Ok(root) => root,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                false,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    if std::fs::create_dir_all(&managed_root).is_err() {
        return managed_worktree_error(
            start,
            "worktree_creation_failed",
            false,
            Some(&base_ref),
            Some(&base_sha),
            Some(source_dirty),
        );
    }
    let managed_root = match canonicalize_existing(&managed_root) {
        Ok(root) => root,
        Err(_) => {
            return managed_worktree_error(
                start,
                "worktree_creation_failed",
                false,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    if validate_project_path_policy(policy, &managed_root).is_err() {
        return managed_worktree_error(
            start,
            "managed_worktree_root_unavailable",
            false,
            Some(&base_ref),
            Some(&base_sha),
            Some(source_dirty),
        );
    }
    let destination = managed_root.join(format!(
        "{}-{}",
        sanitized_project_basename(&source_root),
        operation_id
    ));
    let mut created_worktree = false;
    let canonical_worktree = if destination.exists() {
        let Ok(existing) = canonicalize_existing(&destination) else {
            return managed_worktree_error(
                start,
                "managed_worktree_recovery_conflict",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            );
        };
        let head = match managed_worktree_git_text(&existing, &["rev-parse", "HEAD"]) {
            Ok(head) => head,
            Err(_) => {
                return managed_worktree_error(
                    start,
                    "operation_indeterminate",
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        };
        let listed = match source_mentions_worktree_path(&source_root, &existing) {
            Ok(listed) => listed,
            Err(_) => {
                return managed_worktree_error(
                    start,
                    "operation_indeterminate",
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        };
        if head != base_sha || !listed {
            return managed_worktree_error(
                start,
                "managed_worktree_recovery_conflict",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            );
        }
        existing
    } else {
        match source_mentions_worktree_path(&source_root, &destination) {
            Ok(true) => {
                return managed_worktree_error(
                    start,
                    "managed_worktree_recovery_conflict",
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
            Ok(false) => {}
            Err(_) => {
                return managed_worktree_error(
                    start,
                    "operation_indeterminate",
                    false,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        }
        let destination_string = managed_worktree_git_cli_path(&destination);
        let add_result = run_git_bounded(
            &source_root,
            &[
                "worktree",
                "add",
                "--detach",
                destination_string.as_str(),
                base_sha.as_str(),
            ],
            MANAGED_WORKTREE_GIT_TIMEOUT,
            None,
        );
        match &add_result {
            Ok(output) if output.status.success() => created_worktree = true,
            Ok(_) if !destination.exists() => {
                return match source_mentions_worktree_path(&source_root, &destination) {
                    Ok(false) => managed_worktree_error(
                        start,
                        "worktree_creation_failed",
                        false,
                        Some(&base_ref),
                        Some(&base_sha),
                        Some(source_dirty),
                    ),
                    Ok(true) => managed_worktree_error(
                        start,
                        "managed_worktree_recovery_conflict",
                        true,
                        Some(&base_ref),
                        Some(&base_sha),
                        Some(source_dirty),
                    ),
                    Err(_) => managed_worktree_error(
                        start,
                        "operation_indeterminate",
                        false,
                        Some(&base_ref),
                        Some(&base_sha),
                        Some(source_dirty),
                    ),
                };
            }
            Err(_) if !destination.exists() => {
                return match source_mentions_worktree_path(&source_root, &destination) {
                    Ok(true) => managed_worktree_error(
                        start,
                        "managed_worktree_recovery_conflict",
                        true,
                        Some(&base_ref),
                        Some(&base_sha),
                        Some(source_dirty),
                    ),
                    Ok(false) | Err(_) => managed_worktree_error(
                        start,
                        "operation_indeterminate",
                        false,
                        Some(&base_ref),
                        Some(&base_sha),
                        Some(source_dirty),
                    ),
                };
            }
            Ok(_) | Err(_) => {}
        }
        let worktree = match canonicalize_existing(&destination) {
            Ok(worktree) => worktree,
            Err(_) => {
                return managed_worktree_error(
                    start,
                    "operation_indeterminate",
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        };
        let head = match managed_worktree_git_text(&worktree, &["rev-parse", "HEAD"]) {
            Ok(head) => head,
            Err(_) => {
                return managed_worktree_error(
                    start,
                    "operation_indeterminate",
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        };
        let listed = match source_mentions_worktree_path(&source_root, &worktree) {
            Ok(listed) => listed,
            Err(_) => {
                return managed_worktree_error(
                    start,
                    "operation_indeterminate",
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        };
        if head != base_sha || !listed {
            return managed_worktree_error(
                start,
                "managed_worktree_recovery_conflict",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            );
        }
        worktree
    };
    if validate_project_path_policy(policy, &canonical_worktree).is_err() {
        return managed_worktree_error(
            start,
            "managed_worktree_root_unavailable",
            true,
            Some(&base_ref),
            Some(&base_sha),
            Some(source_dirty),
        );
    }
    let projects = match load_project_files_for_path_resolution(project_registry_dir) {
        Ok(projects) => projects,
        Err(error) => {
            return managed_worktree_error(
                start,
                error,
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    let matches = projects_matching_canonical_path(&projects, &canonical_worktree);
    if matches.len() > 1 {
        return managed_worktree_error(
            start,
            "ambiguous_project_path",
            true,
            Some(&base_ref),
            Some(&base_sha),
            Some(source_dirty),
        );
    }
    if let Some(project) = matches.into_iter().next() {
        let same_operation = project.managed_worktree
            && project.managed_operation_id.as_deref() == Some(operation_id)
            && project.managed_base_sha.as_deref() == Some(base_sha.as_str())
            && project.managed_source.as_deref() == source_root.to_str()
            && project.managed_source_project_id.as_deref() == Some(source_project_id.as_str())
            && project.managed_source_root_fingerprint.as_deref()
                == Some(source_root_fingerprint.as_str());
        if project.disabled || !same_operation {
            return managed_worktree_error(
                start,
                "managed_worktree_recovery_conflict",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            );
        }
        return managed_worktree_success(
            start,
            client_id,
            &project,
            &canonical_worktree,
            &base_ref,
            &base_sha,
            source_dirty,
            "managed_worktree_recovered",
            false,
            false,
        );
    }
    let project_id =
        match choose_auto_project_id(project_registry_dir, &projects, &canonical_worktree) {
            Ok(id) => id,
            Err(error) => {
                return managed_worktree_error(
                    start,
                    error,
                    true,
                    Some(&base_ref),
                    Some(&base_sha),
                    Some(source_dirty),
                )
            }
        };
    let Some(worktree_string) = canonical_worktree.to_str() else {
        return managed_worktree_error(
            start,
            "invalid_project_path",
            true,
            Some(&base_ref),
            Some(&base_sha),
            Some(source_dirty),
        );
    };
    let source_string = source_root.to_str().expect("validated UTF-8 source path");
    let content = managed_worktree_project_toml(
        &project_id,
        &bounded_project_name(&canonical_worktree),
        worktree_string,
        source_string,
        &source_project_id,
        &source_root_fingerprint,
        &base_ref,
        &base_sha,
        operation_id,
    );
    match write_project_toml_atomic(project_registry_dir, &project_id, &content, false) {
        Ok(_) => {}
        Err(ProjectTomlWriteError::BeforeRename) => {
            return managed_worktree_error(
                start,
                "worktree_registration_failed",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
        Err(ProjectTomlWriteError::AfterRename) => {
            return managed_worktree_error(
                start,
                "operation_indeterminate",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    }
    let project = match parse_runner_project_toml(&content) {
        Ok(project) => project,
        Err(_) => {
            return managed_worktree_error(
                start,
                "operation_indeterminate",
                true,
                Some(&base_ref),
                Some(&base_sha),
                Some(source_dirty),
            )
        }
    };
    managed_worktree_success(
        start,
        client_id,
        &project,
        &canonical_worktree,
        &base_ref,
        &base_sha,
        source_dirty,
        if created_worktree {
            "managed_worktree_created"
        } else {
            "managed_worktree_recovered"
        },
        true,
        true,
    )
}

fn lifecycle_config_path(project_registry_dir: &Path, id: &str) -> Result<PathBuf, String> {
    validate_project_op_id(id)?;
    let canonical_dir = canonicalize_existing(project_registry_dir)?;
    let path = canonical_dir.join(format!("{id}.toml"));
    if !path.starts_with(&canonical_dir) {
        return Err("project config path would escape project_registry_dir".to_string());
    }
    Ok(path)
}

fn write_existing_project_atomic(path: &Path, content: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "project config has no parent".to_string())?;
    let id = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("project");
    let temp = unique_registry_temp(dir, id, "toml.tmp");
    let result = (|| {
        let mut file = std::fs::File::create(&temp)
            .map_err(|e| format!("failed to create lifecycle temp file: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("failed to write lifecycle temp file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("failed to sync lifecycle temp file: {e}"))?;
        std::fs::rename(&temp, path)
            .map_err(|e| format!("failed to atomically replace project config: {e}"))?;
        sync_parent_dir(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn cleanup_unregister_tombstones(project_registry_dir: &Path, id: &str) -> Result<(), String> {
    let prefix = format!(".{id}.");
    let suffix = ".toml.unregistering";
    let mut changed = false;
    for entry in std::fs::read_dir(project_registry_dir)
        .map_err(|e| format!("failed to inspect project registry tombstones: {e}"))?
    {
        let entry = entry.map_err(|e| format!("failed to inspect project registry entry: {e}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(suffix) {
            std::fs::remove_file(entry.path())
                .map_err(|e| format!("failed to remove stale unregister tombstone: {e}"))?;
            changed = true;
        }
    }
    if changed {
        sync_dir(project_registry_dir)?;
    }
    Ok(())
}

fn unregister_project_config(path: &Path) -> Result<(), ProjectUnregisterError> {
    let dir = path.parent().ok_or(ProjectUnregisterError::BeforeRename)?;
    let id = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("project");
    let tombstone = unique_registry_temp(dir, id, "toml.unregistering");
    std::fs::rename(path, &tombstone).map_err(|_| ProjectUnregisterError::BeforeRename)?;
    sync_project_parent_after_rename(path).map_err(|_| ProjectUnregisterError::AfterRename)?;
    std::fs::remove_file(&tombstone).map_err(|_| ProjectUnregisterError::AfterRename)?;
    sync_project_parent_after_rename(path).map_err(|_| ProjectUnregisterError::AfterRename)
}

/// Structured, non-shell project lifecycle mutation. Unregister only removes
/// the registry TOML and never touches the project path or Git data.
pub(crate) fn handle_project_lifecycle_operation(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    operation: &RunnerProjectOperation,
) -> CommandResult {
    let _registry_guard = match project_registry_write_lock().lock() {
        Ok(guard) => guard,
        Err(_) => return project_error_cmd(Instant::now(), "operation_failed"),
    };
    let start = Instant::now();
    let action = match operation.kind {
        RunnerProjectOperationKind::LifecycleEnable => "enable",
        RunnerProjectOperationKind::LifecycleDisable => "disable",
        RunnerProjectOperationKind::LifecycleUnregister => "unregister",
        _ => return project_error_cmd(start, "unsupported_runner_version"),
    };
    let payload: serde_json::Value = match serde_json::from_str(&operation.payload).ok() {
        Some(v) => v,
        None => return project_error_cmd(start, "invalid_request"),
    };
    let id = match payload.get("project_id").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => return project_error_cmd(start, "invalid_request"),
    };
    let expected_revision = match payload.get("expected_revision").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => return project_error_cmd(start, "invalid_request"),
    };
    let config_path = match lifecycle_config_path(project_registry_dir, id) {
        Ok(v) => v,
        Err(e) => return err_cmd(start, e),
    };
    if !config_path.exists() {
        if action == "unregister" {
            if cleanup_unregister_tombstones(project_registry_dir, id).is_err() {
                return project_error_cmd(start, "operation_failed");
            }
            return ok_cmd(
                start,
                serde_json::json!({
                    "operation": action, "agent_project_id": id,
                    "outcome": "already_unregistered", "changed": false,
                    "revision": serde_json::Value::Null
                }),
            );
        }
        return project_error_cmd(start, "project_not_found");
    }
    let content = match std::fs::read_to_string(&config_path) {
        Ok(v) => v,
        Err(_) => return project_error_cmd(start, "operation_failed"),
    };
    let mut project = match parse_runner_project_toml(&content) {
        Ok(v) => v,
        Err(_) => return project_error_cmd(start, "operation_failed"),
    };
    let current_revision = project_revision(&project);
    let desired_disabled = action == "disable";
    if action != "unregister" && project.disabled == desired_disabled {
        return ok_cmd(
            start,
            serde_json::json!({
                "operation": action, "agent_project_id": id,
                "outcome": if desired_disabled {"already_disabled"} else {"already_enabled"},
                "changed": false, "revision": current_revision,
                "disabled": project.disabled, "path": project.path,
                "name": project.name, "kind": project.kind,
                "registration_source": effective_registration_source(&project).as_str(),
                "description": project.description,
                "allow_patch": project.allow_patch,
                "root_fingerprint": canonicalize_existing(Path::new(&project.path)).ok()
                    .filter(|path| path.is_dir()).as_deref().map(project_root_fingerprint),
                "lineage": project_lineage(&project)
            }),
        );
    }
    if expected_revision != current_revision {
        return project_error_cmd(start, "revision_conflict");
    }
    if action == "unregister" {
        match unregister_project_config(&config_path) {
            Ok(()) => {}
            Err(ProjectUnregisterError::BeforeRename) => {
                return project_error_cmd(start, "operation_failed")
            }
            Err(ProjectUnregisterError::AfterRename) => {
                return structured_project_error_cmd(
                    start,
                    "operation_indeterminate",
                    true,
                    serde_json::json!({}),
                )
            }
        }
        return ok_cmd(
            start,
            serde_json::json!({
                "operation": action, "agent_project_id": id,
                "outcome": "unregistered", "changed": true,
                "revision": serde_json::Value::Null
            }),
        );
    }
    if !desired_disabled {
        let canonical = match canonicalize_existing(Path::new(&project.path)) {
            Ok(v) if v.is_dir() => v,
            _ => return project_error_cmd(start, "project_not_found"),
        };
        if let Err(error_kind) = validate_windows_project_root(&canonical) {
            return project_error_cmd(start, error_kind);
        }
        if validate_project_path_policy(policy, &canonical).is_err() {
            return project_error_cmd(start, "path_outside_allowed_roots");
        }
    }
    project.disabled = desired_disabled;
    let serialized = match toml::to_string_pretty(&project) {
        Ok(v) => v,
        Err(_) => return project_error_cmd(start, "operation_failed"),
    };
    if write_existing_project_atomic(&config_path, &serialized).is_err() {
        return project_error_cmd(start, "operation_failed");
    }
    let revision = project_revision(&project);
    ok_cmd(
        start,
        serde_json::json!({
            "operation": action, "agent_project_id": id,
            "outcome": if desired_disabled {"disabled"} else {"enabled"},
            "changed": true, "revision": revision,
            "disabled": project.disabled, "path": project.path,
            "name": project.name, "kind": project.kind,
            "registration_source": effective_registration_source(&project).as_str(),
            "description": project.description,
            "allow_patch": project.allow_patch,
            "root_fingerprint": canonicalize_existing(Path::new(&project.path)).ok()
                .filter(|path| path.is_dir()).as_deref().map(project_root_fingerprint),
            "lineage": project_lineage(&project)
        }),
    )
}

fn matching_existing_project(
    project_registry_dir: &Path,
    id: &str,
    name: &str,
    path: &str,
    description: Option<&str>,
    allow_patch: bool,
) -> Result<Option<RunnerProjectFile>, &'static str> {
    let config_path = project_registry_dir.join(format!("{id}.toml"));
    if !config_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&config_path).map_err(|_| "operation_failed")?;
    let project = parse_runner_project_toml(&content).map_err(|_| "operation_failed")?;
    let matches = project.id == id
        && paths_equal(Path::new(&project.path), Path::new(path))
        && project.name.as_deref() == Some(name)
        && project.description.as_deref() == description
        && project.allow_patch == allow_patch
        && !project.disabled;
    if matches {
        Ok(Some(project))
    } else {
        Err("project_already_exists")
    }
}

fn validate_recovered_create_side_effects(
    path: &Path,
    template: &str,
    git_init: bool,
) -> Result<(), &'static str> {
    if !path.is_dir() {
        return Err("project_already_exists");
    }
    if git_init && !path.join(".git").is_dir() {
        return Err("project_already_exists");
    }
    if template == "basic"
        && (!path.join("README.md").is_file() || !path.join(".gitignore").is_file())
    {
        return Err("project_already_exists");
    }
    Ok(())
}

fn recovered_project_result(
    create: bool,
    runtime_id: &str,
    client_id: &str,
    project: &RunnerProjectFile,
    template: Option<&str>,
    git_init: bool,
) -> serde_json::Value {
    let root_fingerprint = canonicalize_existing(Path::new(&project.path))
        .ok()
        .filter(|path| path.is_dir())
        .as_deref()
        .map(project_root_fingerprint);
    serde_json::json!({
        "id": runtime_id, "agent_project_id": project.id, "client_id": client_id,
        "name": project.name, "path": project.path, "kind": project.kind,
        "registration_source": effective_registration_source(project).as_str(),
        "description": project.description,
        "created_directory": false, "created_config": false, "overwritten": false,
        "allow_patch": project.allow_patch, "template": template,
        "git_initialized": git_init, "recovered": true, "changed": false,
        "operation": if create { "create" } else { "register" },
        "outcome": if create { "created" } else { "registered" },
        "revision": project_revision(project),
        "root_fingerprint": root_fingerprint,
        "lineage": project_lineage(project),
    })
}

/// Handle `register_project` / `create_project` agent requests. Parses the
/// JSON payload from `request.stdin`, validates fields and path against
/// policy, writes `project_registry_dir/<id>.toml` atomically (and for
/// `create_project` creates the directory / templates / optional git init),
/// and returns structured JSON in `CommandResult.stdout`.
pub(crate) fn handle_project_operation(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    client_id: &str,
    operation: &RunnerProjectOperation,
) -> CommandResult {
    let _registry_guard = match project_registry_write_lock().lock() {
        Ok(guard) => guard,
        Err(_) => return project_error_cmd(Instant::now(), "operation_failed"),
    };
    let start = Instant::now();
    let kind = operation.kind.wire_kind();
    let create = operation.kind == RunnerProjectOperationKind::Create;
    let payload = match operation.payload.as_str() {
        s if !s.is_empty() => s,
        _ => {
            return CommandResult {
                exit_code: None,
                stdout: None,
                stderr: None,
                duration_ms: Some(start.elapsed().as_millis() as u64),
                error: Some(format!("{} request missing stdin payload", kind)),
            };
        }
    };
    let json: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(e) => {
            return CommandResult {
                exit_code: None,
                stdout: None,
                stderr: None,
                duration_ms: Some(start.elapsed().as_millis() as u64),
                error: Some(format!("failed to parse {} payload: {}", kind, e)),
            };
        }
    };
    if json.get("managed_temporary_project").is_some() {
        return project_error_cmd(start, "managed_temporary_projects_retired");
    }
    let get_str = |key: &str| -> Result<String, String> {
        json.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("{} missing required field '{}'", kind, key))
    };
    let id = match get_str("id") {
        Ok(v) => v,
        Err(e) => return err_cmd(start, e),
    };
    let name = match get_str("name") {
        Ok(v) => v,
        Err(e) => return err_cmd(start, e),
    };
    let path = match get_str("path") {
        Ok(v) => v,
        Err(e) => return err_cmd(start, e),
    };
    let description = json
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let allow_patch = json
        .get("allow_patch")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let overwrite = json
        .get("overwrite")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if let Err(e) = validate_project_op_id(&id) {
        return err_cmd(start, e);
    }
    if let Err(e) = validate_project_op_name(&name) {
        return err_cmd(start, e);
    }
    if let Some(ref desc) = description {
        if let Err(e) = validate_project_op_description(desc) {
            return err_cmd(start, e);
        }
    }
    // `Path::is_absolute` is platform-correct: drive-letter and UNC paths
    // (`C:\foo`, `\\server\share`) are absolute on Windows; bare `foo` or
    // drive-relative `/foo` are not.
    if path.is_empty() || path.contains('\0') || !Path::new(&path).is_absolute() {
        return err_cmd(start, "path must be a non-empty absolute path".to_string());
    }
    // Existing project registration accepts local disks and network shares;
    // special Windows namespaces still fail before filesystem access.
    if let Err(error_kind) = validate_windows_project_root(Path::new(&path)) {
        return project_error_cmd(start, error_kind);
    }
    if !create {
        if let Err(error_kind) =
            validate_model_network_project_ingress_authority(policy, Path::new(&path))
        {
            return project_error_cmd(start, error_kind);
        }
    }
    #[cfg(windows)]
    if create && webcodex_runner_config::paths::is_windows_network_share_path(Path::new(&path)) {
        // Network project creation is deliberately outside this phase. Existing
        // network directories can be registered when RunnerPolicy already grants them.
        return project_error_cmd(start, "windows_project_path_unsupported");
    }

    let client_id = client_id.to_string();
    let runtime_id = format!("agent:{}:{}", client_id, id);

    let toml_content = build_project_toml(&id, &name, &path, &description, allow_patch);
    let template = json
        .get("template")
        .and_then(|v| v.as_str())
        .unwrap_or("empty")
        .to_string();
    let git_init = json
        .get("git_init")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let adopt_existing_empty = json
        .get("adopt_existing_empty")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if create && template != "empty" && template != "basic" {
        return project_error_cmd(start, "invalid_request");
    }

    if !create {
        // The directory must exist and be a directory.
        let path_buf = PathBuf::from(&path);
        let canonical = match path_buf.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                return err_cmd(
                    start,
                    format!(
                        "path does not exist or cannot be canonicalized: {}: {}",
                        path, e
                    ),
                );
            }
        };
        if !canonical.is_dir() {
            return err_cmd(start, format!("path {} is not a directory", path));
        }
        if let Err(error_kind) = validate_windows_project_root(&canonical) {
            return project_error_cmd(start, error_kind);
        }
        if validate_project_path_policy(policy, &canonical).is_err() {
            return project_error_cmd(start, "path_outside_allowed_roots");
        }
        if !overwrite {
            match matching_existing_project(
                project_registry_dir,
                &id,
                &name,
                &path,
                description.as_deref(),
                allow_patch,
            ) {
                Ok(Some(project)) => {
                    return ok_cmd(
                        start,
                        recovered_project_result(
                            create,
                            &runtime_id,
                            &client_id,
                            &project,
                            None,
                            false,
                        ),
                    )
                }
                Ok(None) => {}
                Err(code) => return project_error_cmd(start, code),
            }
        }
        let write_result =
            match write_project_toml_atomic(project_registry_dir, &id, &toml_content, overwrite) {
                Ok(p) => p,
                Err(ProjectTomlWriteError::BeforeRename) => {
                    return project_error_cmd(start, "operation_failed")
                }
                Err(ProjectTomlWriteError::AfterRename) => {
                    return project_error_cmd(start, "operation_indeterminate")
                }
            };
        let result = serde_json::json!({
            "id": runtime_id,
            "agent_project_id": id,
            "client_id": client_id,
            "name": name,
            "path": path,
            "description": description,
            "project_record_path": write_result.config_path.to_string_lossy(),
            "projects_config_path": write_result.config_path.to_string_lossy(),
            "created_config": write_result.created_config,
            "overwritten": write_result.overwritten,
            "allow_patch": allow_patch,
            "registration_source": EXPLICIT_REGISTRATION_SOURCE,
            "revision": project_revision(&parse_runner_project_toml(&toml_content).expect("generated project TOML must parse")),
            "operation": "register", "outcome": "registered", "changed": true, "recovered": false,
        });
        return ok_cmd(start, result);
    }

    // create_project
    let path_buf = PathBuf::from(&path);
    let mut created_directory = false;
    let mut created_paths = CreatedProjectPaths::default();

    // Determine the canonical parent for policy validation. If the path exists,
    // canonicalize it directly. If not, canonicalize the existing ancestor.
    let canonical_for_policy = if path_buf.exists() {
        match path_buf.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                return err_cmd(
                    start,
                    format!("path cannot be canonicalized: {}: {}", path, e),
                );
            }
        }
    } else {
        // Find the nearest existing ancestor and canonicalize it.
        let mut ancestor = path_buf.clone();
        while !ancestor.exists() {
            if let Some(parent) = ancestor.parent() {
                ancestor = parent.to_path_buf();
            } else {
                break;
            }
        }
        match ancestor.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                return err_cmd(
                    start,
                    format!(
                        "parent path cannot be canonicalized: {}: {}",
                        ancestor.display(),
                        e
                    ),
                );
            }
        }
    };
    if let Err(error_kind) = validate_windows_project_root(&canonical_for_policy) {
        return project_error_cmd(start, error_kind);
    }
    #[cfg(windows)]
    if webcodex_runner_config::paths::is_windows_network_share_path(&canonical_for_policy) {
        // A mapped drive may canonicalize to VerbatimUNC. Keep create_project
        // local-only even when the raw spelling looked like a drive letter.
        return project_error_cmd(start, "windows_project_path_unsupported");
    }
    if validate_project_path_policy(policy, &canonical_for_policy).is_err() {
        return project_error_cmd(start, "path_outside_allowed_roots");
    }
    if !overwrite {
        match matching_existing_project(
            project_registry_dir,
            &id,
            &name,
            &path,
            description.as_deref(),
            allow_patch,
        ) {
            Ok(Some(project)) => {
                if let Err(code) =
                    validate_recovered_create_side_effects(&path_buf, &template, git_init)
                {
                    return project_error_cmd(start, code);
                }
                return ok_cmd(
                    start,
                    recovered_project_result(
                        create,
                        &runtime_id,
                        &client_id,
                        &project,
                        Some(&template),
                        git_init,
                    ),
                );
            }
            Ok(None) => {}
            Err(code) => return project_error_cmd(start, code),
        }
    }

    // Handle existing vs new directory.
    if path_buf.exists() {
        let meta = match std::fs::metadata(&path_buf) {
            Ok(m) => m,
            Err(e) => return err_cmd(start, format!("failed to stat path {}: {}", path, e)),
        };
        if !meta.is_dir() {
            return err_cmd(
                start,
                format!("path {} exists but is not a directory", path),
            );
        }
        // Check if the directory is empty.
        let is_empty = match std::fs::read_dir(&path_buf) {
            Ok(mut it) => it.next().is_none(),
            Err(e) => {
                return err_cmd(start, format!("failed to read directory {}: {}", path, e));
            }
        };
        if !is_empty {
            return project_error_cmd(start, "path_not_empty");
        }
        if !adopt_existing_empty {
            return project_error_cmd(start, "path_exists");
        }
    } else {
        // Create the directory.
        if let Err(e) = std::fs::create_dir_all(&path_buf) {
            return err_cmd(start, format!("failed to create directory {}: {}", path, e));
        }
        created_directory = true;
        created_paths.mark_project_dir_created(path_buf.clone());
    }

    // Apply template.
    if template == "basic" {
        let readme = if let Some(ref desc) = description {
            format!("# {}\n\n{}\n", name, desc)
        } else {
            format!("# {}\n", name)
        };
        let readme_path = path_buf.join("README.md");
        if let Err(e) = write_created_file(&readme_path, readme.as_bytes(), &mut created_paths) {
            created_paths.cleanup();
            return err_cmd(start, format!("failed to write README.md: {}", e));
        }
        let gitignore = "target/\nnode_modules/\n.env\n*.log\n";
        let gitignore_path = path_buf.join(".gitignore");
        if let Err(e) =
            write_created_file(&gitignore_path, gitignore.as_bytes(), &mut created_paths)
        {
            created_paths.cleanup();
            return err_cmd(start, format!("failed to write .gitignore: {}", e));
        }
    }
    // `empty` itself generates no project files. Description stays registration
    // metadata; `git_init` remains a separate explicit filesystem side effect.

    // git init.
    let mut git_initialized = false;
    if git_init {
        match run_git_bounded(&path_buf, &["init"], Duration::from_secs(5), None) {
            Ok(output) if output.status.success() => {
                git_initialized = true;
                created_paths.track(path_buf.join(".git"));
            }
            Ok(output) => {
                created_paths.cleanup();
                let stderr = String::from_utf8_lossy(&output.stderr);
                let suffix = if output.stderr_capped {
                    " [stderr truncated]"
                } else {
                    ""
                };
                return err_cmd(
                    start,
                    format!("git init failed: {}{}", stderr.trim(), suffix),
                );
            }
            Err(e) => {
                created_paths.cleanup();
                return err_cmd(start, format!("git init failed (is git installed?): {}", e));
            }
        }
    }

    // Write project TOML.
    let write_result =
        match write_project_toml_atomic(project_registry_dir, &id, &toml_content, overwrite) {
            Ok(p) => p,
            Err(ProjectTomlWriteError::BeforeRename) => {
                created_paths.cleanup();
                return project_error_cmd(start, "operation_failed");
            }
            Err(ProjectTomlWriteError::AfterRename) => {
                return project_error_cmd(start, "operation_indeterminate");
            }
        };
    let result = serde_json::json!({
        "id": runtime_id,
        "agent_project_id": id,
        "client_id": client_id,
        "name": name,
        "path": path,
        "description": description,
        "project_record_path": write_result.config_path.to_string_lossy(),
        "projects_config_path": write_result.config_path.to_string_lossy(),
        "created_directory": created_directory,
        "created_config": write_result.created_config,
        "overwritten": write_result.overwritten,
        "allow_patch": allow_patch,
        "registration_source": EXPLICIT_REGISTRATION_SOURCE,
        "template": template,
        "revision": project_revision(&parse_runner_project_toml(&toml_content).expect("generated project TOML must parse")),
        "git_initialized": git_initialized,
        "operation": "create", "outcome": "created", "changed": true, "recovered": false,
    });
    ok_cmd(start, result)
}

#[cfg(test)]
fn test_project_operation(
    request: &RunnerRequest,
) -> Result<RunnerProjectOperation, CommandResult> {
    match request.decode_operation() {
        Ok(RunnerOperation::Project(operation)) => Ok(operation),
        _ => Err(project_error_cmd(
            Instant::now(),
            "unsupported_runner_version",
        )),
    }
}

#[cfg(test)]
pub(crate) fn handle_project_op(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    request: &RunnerRequest,
) -> CommandResult {
    let operation = match test_project_operation(request) {
        Ok(operation) => operation,
        Err(result) => return result,
    };
    handle_project_operation(policy, project_registry_dir, &request.client_id, &operation)
}

#[cfg(test)]
pub(crate) fn handle_resolve_or_register_project(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    request: &RunnerRequest,
) -> CommandResult {
    let operation = match test_project_operation(request) {
        Ok(operation) => operation,
        Err(result) => return result,
    };
    handle_resolve_or_register_project_operation(
        policy,
        project_registry_dir,
        &request.client_id,
        &operation,
    )
}

#[cfg(test)]
pub(crate) fn handle_prepare_managed_worktree(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    request: &RunnerRequest,
) -> CommandResult {
    let operation = match test_project_operation(request) {
        Ok(operation) => operation,
        Err(result) => return result,
    };
    handle_prepare_managed_worktree_operation(
        policy,
        project_registry_dir,
        &request.client_id,
        &operation,
    )
}

#[cfg(test)]
pub(crate) fn handle_project_lifecycle_op(
    policy: &RunnerPolicy,
    project_registry_dir: &Path,
    request: &RunnerRequest,
) -> CommandResult {
    let operation = match test_project_operation(request) {
        Ok(operation) => operation,
        Err(result) => return result,
    };
    handle_project_lifecycle_operation(policy, project_registry_dir, &operation)
}

#[cfg(test)]
mod durability_tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn managed_worktree_git_cli_path_normalizes_only_verbatim_local_disk_paths() {
        assert_eq!(
            managed_worktree_git_cli_path(Path::new(r"\\?\C:\workspace\managed")),
            r"C:\workspace\managed"
        );
        assert_eq!(
            managed_worktree_git_cli_path(Path::new(r"C:\workspace\managed")),
            r"C:\workspace\managed"
        );
        assert_eq!(
            managed_worktree_git_cli_path(Path::new(r"\\server\share\managed")),
            r"\\server\share\managed"
        );
    }

    /// Unix-only: verifies the POSIX directory-fsync contract (opening a
    /// directory as a file). On Windows directory sync is intentionally
    /// skipped because std cannot open directories with the required
    /// FILE_FLAG_BACKUP_SEMANTICS.
    #[cfg(unix)]
    #[test]
    fn registry_parent_sync_failures_are_not_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing").join("demo.toml");
        let error = sync_parent_dir(&missing).unwrap_err();
        assert!(error.contains("sync project registry directory"));
    }

    #[test]
    fn registry_loader_ignores_temp_and_unregister_tombstones() {
        let tmp = tempfile::tempdir().unwrap();
        let project_registry_dir = tmp.path().join("project-registry");
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&project_registry_dir).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        let content = build_project_toml("demo", "Demo", source.to_str().unwrap(), &None, true);
        std::fs::write(project_registry_dir.join("demo.toml"), &content).unwrap();
        std::fs::write(project_registry_dir.join(".demo.random.toml.tmp"), &content).unwrap();
        std::fs::write(
            project_registry_dir.join(".demo.random.toml.unregistering"),
            &content,
        )
        .unwrap();
        let projects = load_runner_project_summaries_from_dir(&project_registry_dir);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "demo");
    }

    #[cfg(unix)]
    #[test]
    fn project_summary_reports_retargeted_symlinks_as_distinct_canonical_roots() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        let link = tmp.path().join("current");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        symlink(&first, &link).unwrap();
        let project = RunnerProjectFile {
            id: "demo".to_string(),
            path: link.to_string_lossy().to_string(),
            shell_profile: None,
            allow_patch: true,
            name: None,
            kind: None,
            registration_source: None,
            description: None,
            disabled: false,
            hooks: HashMap::new(),
            managed_worktree: false,
            managed_source: None,
            managed_source_project_id: None,
            managed_source_root_fingerprint: None,
            managed_base_ref: None,
            managed_base_sha: None,
            managed_operation_id: None,
        };

        let first_summary = runner_project_summary(&project, 1, false);
        assert_eq!(
            Path::new(&first_summary.path),
            first.canonicalize().unwrap()
        );
        std::fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        let second_summary = runner_project_summary(&project, 2, false);
        assert_eq!(
            Path::new(&second_summary.path),
            second.canonicalize().unwrap()
        );
        assert_ne!(first_summary.path, second_summary.path);
    }
}
