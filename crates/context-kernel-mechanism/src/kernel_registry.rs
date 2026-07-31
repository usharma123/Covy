use super::*;

/// Loads one JSON packet file into the kernel wire representation.
///
/// # Errors
///
/// Returns [`KernelError::PacketRead`] for I/O failures and
/// [`KernelError::PacketParse`] for invalid JSON.
pub fn load_packet_file(path: &Path) -> Result<KernelPacket, KernelError> {
    let raw = std::fs::read_to_string(path).map_err(|source| KernelError::PacketRead {
        path: path.to_string_lossy().to_string(),
        detail: source.to_string(),
    })?;

    let value: Value = serde_json::from_str(&raw).map_err(|source| KernelError::PacketParse {
        path: path.to_string_lossy().to_string(),
        detail: source.to_string(),
    })?;

    Ok(KernelPacket::from_value(
        value,
        Some(path.to_string_lossy().to_string()),
    ))
}
