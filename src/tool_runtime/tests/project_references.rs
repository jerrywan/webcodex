use super::support::*;
use crate::runner_protocol::{RunnerCapabilities, RunnerProjectSummary};
use crate::tool_runtime::projects::ListProjectsOptions;
use crate::tool_runtime::ToolRuntime;
use std::sync::Arc;

fn root(hex: char) -> String {
    format!("wc_projroot_{}", hex.to_string().repeat(64))
}

fn project(
    client_id: &str,
    id: &str,
    name: &str,
    path: &str,
    fingerprint: char,
) -> RunnerProjectSummary {
    let mut project = named_registered_project(client_id, id, name, path, 1);
    project.root_fingerprint = Some(root(fingerprint));
    project
}

fn project_ref_for(listed: &crate::tool_runtime::ToolResult, canonical: &str) -> String {
    listed.output["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["id"] == canonical)
        .and_then(|project| project["project_ref"].as_str())
        .unwrap()
        .to_string()
}

fn runtime_with_reference_db(path: &std::path::Path) -> ToolRuntime {
    let db = Arc::new(crate::Database::open(&path.to_path_buf()).unwrap());
    ToolRuntime::new_for_tests().with_project_reference_database(db)
}

#[tokio::test]
async fn project_refs_route_exact_projects_without_bare_name_uniqueness() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = runtime_with_reference_db(&tmp.path().join("refs.db"));
    register_agent_projects(
        &runtime,
        "special",
        None,
        RunnerCapabilities::default(),
        vec![
            project(
                "special",
                "webcodex",
                "WebCodex special",
                "/srv/special",
                '1',
            ),
            project("special", "alpha-id", "friendly", "/srv/alpha", '2'),
        ],
    )
    .await;
    register_agent_projects(
        &runtime,
        "oe",
        None,
        RunnerCapabilities::default(),
        vec![project("oe", "webcodex", "WebCodex oe", "/srv/oe", '3')],
    )
    .await;

    let listed = runtime
        .list_projects_with_options(None, ListProjectsOptions::default())
        .await;
    assert!(listed.success, "{:?}", listed.error);
    let special = "agent:special:webcodex";
    let oe = "agent:oe:webcodex";
    let special_ref = project_ref_for(&listed, special);
    let oe_ref = project_ref_for(&listed, oe);
    assert_ne!(special_ref, oe_ref);
    assert!(special_ref.starts_with("~p"));
    assert!(oe_ref.starts_with("~p"));

    assert_eq!(
        runtime
            .resolve_project_input(&special_ref)
            .await
            .unwrap()
            .resolved_id,
        special
    );
    assert_eq!(
        runtime
            .resolve_project_input(&oe_ref)
            .await
            .unwrap()
            .resolved_id,
        oe
    );
    assert!(
        runtime.resolve_project_input("webcodex").await.is_err(),
        "duplicate bare project ids must remain ambiguous"
    );
    assert_eq!(
        runtime
            .resolve_project_input("special:webcodex")
            .await
            .unwrap()
            .resolved_id,
        special
    );
    assert_eq!(
        runtime
            .resolve_project_input(special)
            .await
            .unwrap()
            .resolved_id,
        special
    );
    assert_eq!(
        runtime
            .resolve_project_input("alpha-id")
            .await
            .unwrap()
            .resolved_id,
        "agent:special:alpha-id"
    );
    assert_eq!(
        runtime
            .resolve_project_input("friendly")
            .await
            .unwrap()
            .resolved_id,
        "agent:special:alpha-id"
    );

    let relisted = runtime
        .list_projects_with_options(None, ListProjectsOptions::default())
        .await;
    assert_eq!(project_ref_for(&relisted, special), special_ref);
    assert_eq!(project_ref_for(&relisted, oe), oe_ref);
}

#[tokio::test]
async fn project_refs_are_principal_scoped_and_reauthorize_visibility() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = runtime_with_reference_db(&tmp.path().join("refs.db"));
    register_agent_projects(
        &runtime,
        "private",
        Some("alice"),
        RunnerCapabilities::default(),
        vec![project("private", "repo", "repo", "/srv/private", '4')],
    )
    .await;
    let canonical = "agent:private:repo";
    let alice = auth_context(Some("alice"), false);
    let resolved = runtime
        .resolve_project_input_for_auth(canonical, Some(&alice))
        .await
        .unwrap();
    let project_ref = runtime
        .project_reference_for_resolved(&resolved, Some(&alice))
        .unwrap();

    let bob = auth_context(Some("bob"), false);
    assert!(
        runtime
            .resolve_project_input_for_auth(&project_ref, Some(&bob))
            .await
            .is_err(),
        "another principal must not learn or use Alice's short mapping"
    );

    let mut same_principal_revoked = alice.clone();
    same_principal_revoked.username = Some("bob".to_string());
    assert!(
        runtime
            .resolve_project_input_for_auth(&project_ref, Some(&same_principal_revoked))
            .await
            .is_err(),
        "short ref must rerun current Runner visibility even for the same principal namespace"
    );
}

#[tokio::test]
async fn stale_project_ref_never_retargets_and_survives_store_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("refs.db");
    let canonical = "agent:special:repo";
    let original_ref = {
        let runtime = runtime_with_reference_db(&db_path);
        register_agent_projects(
            &runtime,
            "special",
            None,
            RunnerCapabilities::default(),
            vec![project("special", "repo", "repo", "/srv/original", '5')],
        )
        .await;
        let resolved = runtime.resolve_project_input(canonical).await.unwrap();
        runtime
            .project_reference_for_resolved(&resolved, None)
            .unwrap()
    };

    let runtime = runtime_with_reference_db(&db_path);
    register_agent_projects(
        &runtime,
        "special",
        None,
        RunnerCapabilities::default(),
        vec![project("special", "repo", "repo", "/srv/original", '5')],
    )
    .await;
    assert_eq!(
        runtime
            .resolve_project_input(&original_ref)
            .await
            .unwrap()
            .resolved_id,
        canonical,
        "store reopen must preserve the original mapping"
    );

    crate::test_support::apply_project_inventory_snapshot(
        &runtime.runner_registry,
        "special",
        "inst-special",
        Vec::new(),
    )
    .await;
    assert!(
        runtime.resolve_project_input(&original_ref).await.is_err(),
        "disappearing Project must make its old ref fail closed"
    );

    crate::test_support::apply_project_inventory_snapshot(
        &runtime.runner_registry,
        "special",
        "inst-special",
        vec![project("special", "repo", "repo", "/srv/replacement", '6')],
    )
    .await;
    assert!(
        runtime.resolve_project_input(&original_ref).await.is_err(),
        "same canonical id with a different root must not retarget the old ref"
    );
    assert_eq!(
        runtime
            .resolve_project_input(canonical)
            .await
            .unwrap()
            .resolved_id,
        canonical,
        "canonical full id must continue to address the replacement Project explicitly"
    );
    let replacement = runtime.resolve_project_input(canonical).await.unwrap();
    let replacement_ref = runtime
        .project_reference_for_resolved(&replacement, None)
        .unwrap();
    assert_ne!(replacement_ref, original_ref);
    assert_eq!(
        runtime
            .resolve_project_input(&replacement_ref)
            .await
            .unwrap()
            .resolved_id,
        canonical
    );
}
