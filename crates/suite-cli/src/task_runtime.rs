use std::path::Path;

use anyhow::{Context, Result};
use packet28_daemon_core::storage::{load_active_task_record, save_active_task_record};
use packet28_daemon_protocol::hooks::ActiveTaskRecord;

pub fn load_active_task(root: &Path) -> Result<Option<ActiveTaskRecord>> {
    load_active_task_record(root).context("failed to load bounded active-task state")
}

pub fn store_active_task(root: &Path, record: &ActiveTaskRecord) -> Result<()> {
    save_active_task_record(root, record).context("failed to store bounded active-task state")
}

pub fn derive_claude_task_id(session_id: &str) -> String {
    crate::broker_client::derive_task_id(&format!("claude-session:{session_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet28_daemon_core::storage::MAX_ACTIVE_TASK_RECORD_BYTES;
    use packet28_daemon_protocol::paths::active_task_path;
    use std::fs;
    use tempfile::tempdir;

    fn record_with_encoded_size(target_bytes: usize) -> ActiveTaskRecord {
        let mut record = ActiveTaskRecord {
            task_id: "bounded".to_string(),
            session_id: Some(String::new()),
            updated_at_unix: 1,
        };
        let base_bytes = serde_json::to_vec_pretty(&record).unwrap().len();
        let padding = target_bytes
            .checked_sub(base_bytes)
            .expect("target must fit the base active-task record");
        record.session_id = Some("x".repeat(padding));
        assert_eq!(
            serde_json::to_vec_pretty(&record).unwrap().len(),
            target_bytes
        );
        record
    }

    #[test]
    fn active_task_record_accepts_the_exact_shared_byte_limit() {
        let root = tempdir().unwrap();
        let record = record_with_encoded_size(MAX_ACTIVE_TASK_RECORD_BYTES);

        store_active_task(root.path(), &record).unwrap();
        let loaded = load_active_task(root.path()).unwrap().unwrap();

        assert_eq!(loaded.task_id, "bounded");
        assert_eq!(
            fs::metadata(active_task_path(root.path())).unwrap().len(),
            MAX_ACTIVE_TASK_RECORD_BYTES as u64
        );
    }

    #[test]
    fn oversized_active_task_record_preserves_the_existing_file() {
        let root = tempdir().unwrap();
        let existing = ActiveTaskRecord {
            task_id: "existing".to_string(),
            session_id: None,
            updated_at_unix: 1,
        };
        store_active_task(root.path(), &existing).unwrap();
        let path = active_task_path(root.path());
        let before = fs::read(&path).unwrap();
        let oversized = record_with_encoded_size(MAX_ACTIVE_TASK_RECORD_BYTES + 1);

        let error = store_active_task(root.path(), &oversized).unwrap_err();

        assert!(format!("{error:#}").contains("maximum supported size"));
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(
            load_active_task(root.path()).unwrap().unwrap().task_id,
            "existing"
        );
    }

    #[test]
    fn oversized_persisted_active_task_record_is_not_read() {
        let root = tempdir().unwrap();
        let path = active_task_path(root.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_ACTIVE_TASK_RECORD_BYTES as u64 + 1)
            .unwrap();

        assert!(load_active_task(root.path()).is_err());
    }
}
