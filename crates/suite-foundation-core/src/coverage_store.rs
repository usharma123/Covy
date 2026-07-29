use crate::error::CovyError;
use crate::model::CoverageData;

/// Serialize CoverageData to bytes for storage.
pub fn serialize_coverage(data: &CoverageData) -> Result<Vec<u8>, CovyError> {
    // We store a simplified version since RoaringBitmap isn't directly bincode-serializable
    let mut out = Vec::new();

    // Write file count
    let file_count = data.files.len() as u32;
    out.extend_from_slice(&file_count.to_le_bytes());

    for (path, fc) in &data.files {
        // Write path
        let path_bytes = path.as_bytes();
        out.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(path_bytes);

        // Write covered bitmap
        let mut covered_buf = Vec::new();
        fc.lines_covered
            .serialize_into(&mut covered_buf)
            .map_err(|e| CovyError::Cache(format!("bitmap serialize error: {e}")))?;
        out.extend_from_slice(&(covered_buf.len() as u32).to_le_bytes());
        out.extend_from_slice(&covered_buf);

        // Write instrumented bitmap
        let mut instr_buf = Vec::new();
        fc.lines_instrumented
            .serialize_into(&mut instr_buf)
            .map_err(|e| CovyError::Cache(format!("bitmap serialize error: {e}")))?;
        out.extend_from_slice(&(instr_buf.len() as u32).to_le_bytes());
        out.extend_from_slice(&instr_buf);
    }

    out.extend_from_slice(&data.timestamp.to_le_bytes());
    Ok(out)
}

/// Deserialize CoverageData from bytes.
pub fn deserialize_coverage(data: &[u8]) -> Result<CoverageData, CovyError> {
    use roaring::RoaringBitmap;
    use std::io::Cursor;

    let mut pos = 0;
    let read_u32 = |pos: &mut usize| -> Result<u32, CovyError> {
        read_array::<4>(data, pos).map(u32::from_le_bytes)
    };

    let file_count = read_u32(&mut pos)?;
    let mut files = std::collections::BTreeMap::new();

    for _ in 0..file_count {
        let path_len = read_u32(&mut pos)? as usize;
        if pos + path_len > data.len() {
            return Err(CovyError::Cache("unexpected EOF".to_string()));
        }
        let path = String::from_utf8_lossy(&data[pos..pos + path_len]).to_string();
        pos += path_len;

        let covered_len = read_u32(&mut pos)? as usize;
        if pos + covered_len > data.len() {
            return Err(CovyError::Cache("unexpected EOF".to_string()));
        }
        let lines_covered =
            RoaringBitmap::deserialize_from(Cursor::new(&data[pos..pos + covered_len]))
                .map_err(|e| CovyError::Cache(format!("bitmap deserialize error: {e}")))?;
        pos += covered_len;

        let instr_len = read_u32(&mut pos)? as usize;
        if pos + instr_len > data.len() {
            return Err(CovyError::Cache("unexpected EOF".to_string()));
        }
        let lines_instrumented =
            RoaringBitmap::deserialize_from(Cursor::new(&data[pos..pos + instr_len]))
                .map_err(|e| CovyError::Cache(format!("bitmap deserialize error: {e}")))?;
        pos += instr_len;

        files.insert(
            path,
            crate::model::FileCoverage {
                lines_covered,
                lines_instrumented,
                branches: std::collections::BTreeMap::new(),
                functions: std::collections::BTreeMap::new(),
            },
        );
    }

    let timestamp = read_array::<8>(data, &mut pos)
        .map(u64::from_le_bytes)
        .unwrap_or_default();

    Ok(CoverageData {
        files,
        format: None,
        timestamp,
    })
}

fn read_array<const N: usize>(data: &[u8], pos: &mut usize) -> Result<[u8; N], CovyError> {
    let end = pos
        .checked_add(N)
        .ok_or_else(|| CovyError::Cache("unexpected EOF".to_string()))?;
    let bytes = data
        .get(*pos..end)
        .ok_or_else(|| CovyError::Cache("unexpected EOF".to_string()))?;
    let array = bytes
        .try_into()
        .map_err(|_| CovyError::Cache("unexpected EOF".to_string()))?;
    *pos = end;
    Ok(array)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coverage_serialization_roundtrip() {
        let mut data = CoverageData::new();
        let mut fc = crate::model::FileCoverage::new();
        fc.lines_covered.insert(1);
        fc.lines_covered.insert(5);
        fc.lines_instrumented.insert(1);
        fc.lines_instrumented.insert(2);
        fc.lines_instrumented.insert(5);
        data.files.insert("test.rs".to_string(), fc);

        let bytes = serialize_coverage(&data).unwrap();
        let restored = deserialize_coverage(&bytes).unwrap();
        assert_eq!(restored.files.len(), 1);
        let rfc = &restored.files["test.rs"];
        assert_eq!(rfc.lines_covered.len(), 2);
        assert_eq!(rfc.lines_instrumented.len(), 3);
    }

    #[test]
    fn truncated_file_count_returns_cache_error_without_panicking() {
        let error = deserialize_coverage(&[1, 0, 0]).unwrap_err();

        assert!(matches!(error, CovyError::Cache(message) if message == "unexpected EOF"));
    }
}
