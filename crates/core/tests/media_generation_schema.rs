use rusqlite::Connection;

#[test]
fn media_generation_migration_is_idempotent_and_preserves_integrity_constraints() {
    let conn = Connection::open_in_memory().unwrap();
    nexa_core::migrations::run_migrations(&conn).unwrap();
    nexa_core::migrations::run_migrations(&conn).unwrap();

    for table in [
        "media_jobs",
        "media_job_attempts",
        "media_assets",
        "media_asset_relations",
        "media_provider_events",
        "media_exports",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "{table} must exist after v098");
    }

    let attempt_task_index: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_media_job_attempt_provider_task'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(attempt_task_index.contains("provider_source, provider_task_id"));

    for (table, column) in [
        ("media_jobs", "input_asset_ids_json"),
        ("media_job_attempts", "remote_deletion_status"),
        ("media_job_attempts", "retry_classification"),
        ("media_job_attempts", "next_eligible_at"),
        ("media_job_attempts", "cancellation_requested_at"),
        ("media_job_attempts", "remote_retention_expires_at"),
        ("media_assets", "local_state"),
        ("media_asset_relations", "local_retention_policy"),
        ("media_asset_relations", "local_retention_expires_at"),
    ] {
        let exists: bool = conn
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1)"
                ),
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "{table}.{column} must exist");
    }
    for removed in [
        "source_provider_id",
        "source_model_id",
        "source_api_version",
        "provenance_json",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('media_assets') WHERE name = ?1)",
                [removed],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !exists,
            "blob-intrinsic media_assets must not own {removed}"
        );
    }

    for table in ["media_asset_relations", "media_provider_events"] {
        let attempt_required: bool = conn
            .query_row(
                &format!(
                    "SELECT \"notnull\" = 1 FROM pragma_table_info('{table}') WHERE name = 'attempt_id'"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(attempt_required, "{table}.attempt_id must be required");
    }

    let export_asset_delete_policy: String = conn
        .query_row(
            "SELECT on_delete FROM pragma_foreign_key_list('media_exports') WHERE \"from\" = 'asset_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(export_asset_delete_policy, "RESTRICT");

    let migration_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM _migrations WHERE name = 'v098_media_job_runtime_and_asset_lineage'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migration_count, 1);
}
