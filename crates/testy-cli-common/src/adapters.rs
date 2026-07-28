use std::path::Path;

use suite_packet_core::{CoverageData, CoverageFormat, FileDiff};
use testy_core::error::{AdapterError, AdapterResult};

pub fn default_impact_adapters() -> testy_core::pipeline::ImpactAdapters {
    testy_core::pipeline::ImpactAdapters {
        ingest_coverage_auto,
        ingest_coverage_with_format,
        git_diff: impact_git_diff,
    }
}

pub fn default_testmap_adapters() -> testy_core::pipeline_testmap::TestMapAdapters {
    testy_core::pipeline_testmap::TestMapAdapters {
        ingest_coverage: ingest_coverage_auto,
    }
}

fn ingest_coverage_auto(path: &Path) -> AdapterResult<CoverageData> {
    covy_ingest::ingest_path(path).map_err(Into::into)
}

fn ingest_coverage_with_format(path: &Path, format: CoverageFormat) -> AdapterResult<CoverageData> {
    covy_ingest::ingest_path_with_format(path, format).map_err(Into::into)
}

fn impact_git_diff(base: &str, head: &str) -> AdapterResult<Vec<FileDiff>> {
    diffy_core::diff::git_diff(base, head)
        .map_err(|source| AdapterError::external("Failed to collect git diff", source))
}
