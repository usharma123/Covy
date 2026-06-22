use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(super) struct SystemOutput {
    pub(super) command: String,
    pub(super) summary: String,
    pub(super) text: String,
}

pub(super) fn emit_system_output(
    json_output: bool,
    pretty: bool,
    output: SystemOutput,
) -> Result<i32> {
    if json_output {
        crate::cmd_common::emit_json(&serde_json::to_value(output)?, pretty)?;
    } else {
        print!("{}", output.text);
        if !output.text.ends_with('\n') {
            println!();
        }
    }
    Ok(0)
}

pub(super) fn summarize_rendered_lines(label: &str, text: &str) -> String {
    format!("{label}: {} lines", text.lines().count())
}
