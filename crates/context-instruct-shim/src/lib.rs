#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::*;

fn configured_instruction_mode() -> Option<packet28_daemon_protocol::message::InstructionRenderMode>
{
    let value = std::env::var_os("PACKET28_INSTRUCTION_MODE");
    instruction_mode_from_os_config(value.as_deref())
}

#[cfg(test)]
fn instruction_mode_from_config(
    value: Option<&str>,
) -> Option<packet28_daemon_protocol::message::InstructionRenderMode> {
    instruction_mode_from_os_config(value.map(std::ffi::OsStr::new))
}

fn instruction_mode_from_os_config(
    value: Option<&std::ffi::OsStr>,
) -> Option<packet28_daemon_protocol::message::InstructionRenderMode> {
    value.map(|value| {
        value
            .to_str()
            .and_then(packet28_daemon_protocol::message::InstructionRenderMode::from_config_str)
            .unwrap_or(packet28_daemon_protocol::message::InstructionRenderMode::Passthrough)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet28_daemon_protocol::message::InstructionRenderMode;

    #[test]
    fn absent_instruction_mode_keeps_daemon_default() {
        assert_eq!(instruction_mode_from_config(None), None);
    }

    #[test]
    fn explicit_instruction_modes_are_parsed_case_insensitively() {
        assert_eq!(
            instruction_mode_from_config(Some(" stable ")),
            Some(InstructionRenderMode::Stable)
        );
        assert_eq!(
            instruction_mode_from_config(Some("ADAPTIVE")),
            Some(InstructionRenderMode::Adaptive)
        );
    }

    #[test]
    fn invalid_explicit_mode_fails_open_to_passthrough() {
        assert_eq!(
            instruction_mode_from_config(Some("unknown")),
            Some(InstructionRenderMode::Passthrough)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_explicit_mode_fails_open_to_passthrough() {
        use std::os::unix::ffi::OsStrExt;

        assert_eq!(
            instruction_mode_from_os_config(Some(std::ffi::OsStr::from_bytes(b"\xff"))),
            Some(InstructionRenderMode::Passthrough)
        );
    }
}
