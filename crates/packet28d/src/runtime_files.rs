use super::*;

pub(crate) fn default_index_manifest(root: &Path) -> DaemonIndexManifest {
    DaemonIndexManifest {
        schema_version: INTERACTIVE_INDEX_SCHEMA_VERSION,
        root: root.to_string_lossy().to_string(),
        generation: 0,
        include_tests: true,
        status: DaemonIndexState::Missing,
        dirty_paths: Vec::new(),
        queued_paths: Vec::new(),
        total_files: 0,
        indexed_files: 0,
        regex_generation: None,
        regex_status: None,
        regex_total_files: 0,
        regex_base_commit: None,
        regex_weight_table_version: None,
        regex_stale_reason: None,
        regex_indexed_files: 0,
        last_build_started_at_unix: None,
        last_build_completed_at_unix: None,
        last_error: None,
    }
}

pub(crate) fn load_index_manifest_file(root: &Path) -> DaemonIndexManifest {
    let path = index_manifest_path(root);
    let Ok(raw) = fs::read(&path) else {
        return default_index_manifest(root);
    };
    let Ok(mut manifest) = serde_json::from_slice::<DaemonIndexManifest>(&raw) else {
        return default_index_manifest(root);
    };
    if manifest.schema_version != INTERACTIVE_INDEX_SCHEMA_VERSION {
        return default_index_manifest(root);
    }
    manifest.root = root.to_string_lossy().to_string();
    manifest
}

pub(crate) fn save_index_manifest_file(root: &Path, manifest: &DaemonIndexManifest) -> Result<()> {
    fs::create_dir_all(index_dir(root))
        .with_context(|| format!("failed to create index dir '{}'", index_dir(root).display()))?;
    fs::write(
        index_manifest_path(root),
        serde_json::to_vec_pretty(manifest)?,
    )
    .with_context(|| {
        format!(
            "failed to write index manifest '{}'",
            index_manifest_path(root).display()
        )
    })?;
    Ok(())
}

pub(crate) fn load_index_runtime_files(
    root: &Path,
    manifest: DaemonIndexManifest,
) -> InteractiveIndexRuntime {
    let repo_runtime = mapy_core::load_repo_index_runtime(root).unwrap_or_else(|error| {
        let mut runtime = mapy_core::RepoIndexRuntime::default();
        runtime.manifest.status = "corrupt".to_string();
        runtime.manifest.last_error = Some(error.to_string());
        runtime
    });
    let regex_runtime = packet28_search_core::load_runtime(root).unwrap_or_else(|error| {
        let mut runtime = packet28_search_core::RegexIndexRuntime::default();
        runtime.manifest.status = "corrupt".to_string();
        runtime.manifest.stale_reason = Some(error.to_string());
        runtime.manifest.last_error = Some(error.to_string());
        runtime
    });
    InteractiveIndexRuntime {
        manifest,
        repo_runtime: Some(repo_runtime),
        regex_runtime: Some(regex_runtime),
    }
}

pub(crate) fn clear_index_files(root: &Path) -> Result<()> {
    mapy_core::clear_repo_index_runtime(root)
        .map_err(|error| anyhow!("failed to clear repository index runtime: {error}"))?;
    for path in [index_manifest_path(root), index_snapshot_path(root)] {
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove '{}'", path.display()))?;
        }
    }
    Ok(())
}
