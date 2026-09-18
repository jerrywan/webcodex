use super::{
    ReadFilesItem, ResolvedProject, SearchProjectTextsQuery, SearchResultMode, ToolResult,
    ToolRuntime,
};
use serde_json::{json, Value};

const DEFAULT_READ_CONTEXT: usize = 40;
const MAX_READ_CONTEXT: usize = 100;
const DEFAULT_MAX_READS: usize = 8;
const MAX_READS: usize = 8;

fn match_read_items(
    search_output: &Value,
    read_before: usize,
    read_after: usize,
    max_reads: usize,
) -> Vec<ReadFilesItem> {
    search_output
        .get("matches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|matched| {
            let path = matched.get("path")?.as_str()?;
            let line = usize::try_from(matched.get("line")?.as_u64()?).ok()?;
            let start_line = line.saturating_sub(read_before).max(1);
            let end_line = line.saturating_add(read_after);
            Some(ReadFilesItem {
                path: path.to_string(),
                start_line: Some(start_line),
                limit: Some(end_line.saturating_sub(start_line).saturating_add(1)),
                expected_read_revision: None,
            })
        })
        .take(max_reads)
        .collect()
}

fn first_search_item_output(batch: &Value) -> Option<&Value> {
    let item = batch.get("items")?.as_array()?.first()?;
    item.get("success")
        .and_then(Value::as_bool)
        .filter(|success| *success)
        .and_then(|_| item.get("output"))
}

impl ToolRuntime {
    pub(crate) async fn search_and_read(
        &self,
        project: String,
        query: SearchProjectTextsQuery,
        session_id: Option<String>,
        read_before: Option<usize>,
        read_after: Option<usize>,
        max_reads: Option<usize>,
        with_line_numbers: Option<bool>,
    ) -> ToolResult {
        let resolved = match self.resolve_project_input(&project).await {
            Ok(project) => project,
            Err(error) => return error.into_tool_result(),
        };
        self.search_and_read_resolved(
            &resolved,
            query,
            session_id,
            read_before,
            read_after,
            max_reads,
            with_line_numbers,
        )
        .await
    }

    pub(crate) async fn search_and_read_resolved(
        &self,
        resolved: &ResolvedProject,
        mut query: SearchProjectTextsQuery,
        session_id: Option<String>,
        read_before: Option<usize>,
        read_after: Option<usize>,
        max_reads: Option<usize>,
        with_line_numbers: Option<bool>,
    ) -> ToolResult {
        let read_before = read_before
            .unwrap_or(DEFAULT_READ_CONTEXT)
            .min(MAX_READ_CONTEXT);
        let read_after = read_after
            .unwrap_or(DEFAULT_READ_CONTEXT)
            .min(MAX_READ_CONTEXT);
        let max_reads = max_reads.unwrap_or(DEFAULT_MAX_READS).clamp(1, MAX_READS);

        query.result_mode = Some(SearchResultMode::Matches);
        query.context_before = Some(0);
        query.context_after = Some(0);
        query.limit = Some(query.limit.unwrap_or(max_reads).min(max_reads));

        let search = self
            .search_project_texts_resolved(resolved, vec![query])
            .await;
        if !search.success {
            return search;
        }
        let Some(mut search_output) = first_search_item_output(&search.output).cloned() else {
            return ToolResult::err_with_output(
                "search_and_read could not obtain a successful search result",
                json!({
                    "project": resolved.resolved_id,
                    "search": search.output,
                    "state_changed": false,
                }),
            );
        };
        if let Some(matches) = search_output
            .get_mut("matches")
            .and_then(Value::as_array_mut)
        {
            for matched in matches {
                if let Some(object) = matched.as_object_mut() {
                    object.remove("read_hint");
                }
            }
        }
        let items = match_read_items(&search_output, read_before, read_after, max_reads);
        if items.is_empty() {
            return ToolResult::ok(json!({
                "project": resolved.resolved_id,
                "search": search_output,
                "reads": [],
                "read_request_count": 0,
            }));
        }

        let requested_reads = items.len();
        // Keep original members for canonical byte-ceiling fallback, but return
        // each successful physical union only once. Continuations must follow
        // the actual output ranges, not the optimistic pre-execution plan.
        let (mut reads, output_items) = self
            .read_files_coalesced_resolved(resolved, items, with_line_numbers)
            .await;
        let coalesced_reads = output_items.len();
        let projection = super::read_files::ReadModelProjection::Batch {
            project: resolved.resolved_id.clone(),
            items: output_items,
            session_id,
            with_line_numbers,
            max_result_bytes: Some(super::read_files::DEFAULT_READ_FILES_RESULT_BYTES),
        };
        super::read_files::apply_model_facing_output_budget(
            &mut reads,
            Some(super::read_files::DEFAULT_READ_FILES_RESULT_BYTES),
            &projection,
        );
        super::read_files::enforce_final_model_facing_hard_cap(&mut reads, &projection);
        super::read_files::add_actionable_read_continuations(&projection, &mut reads);
        super::dispatch::sparsify_complete_read_success("read_files", &mut reads);
        ToolResult::ok(json!({
            "project": resolved.resolved_id,
            "search": search_output,
            "reads": reads.output,
            "read_request_count": requested_reads,
            "coalesced_read_count": coalesced_reads,
            "read_success": reads.success,
            "read_error": reads.error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_schema_parses_search_and_read_and_rejects_unknown_fields() {
        let parsed = crate::tool_runtime::ToolCall::from_tool_name(
            "search_and_read",
            json!({
                "project": "demo",
                "query": {"pattern": "needle", "pattern_mode": "literal"},
                "read_before": 20,
                "read_after": 30,
                "max_reads": 4,
                "with_line_numbers": true
            }),
        )
        .unwrap();
        assert!(matches!(
            parsed,
            crate::tool_runtime::ToolCall::SearchAndRead {
                project,
                read_before: Some(20),
                read_after: Some(30),
                max_reads: Some(4),
                with_line_numbers: Some(true),
                ..
            } if project == "demo"
        ));
        assert!(crate::tool_runtime::ToolCall::from_tool_name(
            "search_and_read",
            json!({
                "project": "demo",
                "query": {"pattern": "needle"},
                "unexpected": true
            })
        )
        .is_err());
    }

    #[test]
    fn match_ranges_are_bounded_and_start_at_one() {
        let search = json!({
            "matches": [
                {"path": "src/a.rs", "line": 5},
                {"path": "src/a.rs", "line": 100}
            ]
        });
        let items = match_read_items(&search, 40, 40, 8);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].start_line, Some(1));
        assert_eq!(items[0].limit, Some(45));
        assert_eq!(items[1].start_line, Some(60));
        assert_eq!(items[1].limit, Some(81));
    }

    #[test]
    fn overlapping_match_ranges_are_coalesced_before_compound_read() {
        let search = json!({
            "matches": [
                {"path": "index.vue", "line": 26},
                {"path": "index.vue", "line": 43},
                {"path": "index.vue", "line": 60},
                {"path": "index.vue", "line": 77}
            ]
        });
        let items = match_read_items(&search, 20, 59, 8);
        assert_eq!(items.len(), 4);
        let merged = super::super::read_files::coalesce_read_files_items(items);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_line, Some(6));
        assert_eq!(merged[0].limit, Some(131));
    }
}
