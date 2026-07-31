pub(crate) use crate::runtime_integrations::{
    adapters, detect_runtimes, PromptTarget, RuntimeEnvironment, RuntimeInfo,
};

pub(crate) fn select_setup_runtimes<'a>(
    runtimes: &'a [RuntimeInfo],
    choice: &crate::cmd_setup::SetupPlanChoice,
) -> Vec<&'a RuntimeInfo> {
    match &choice.runtime_scope {
        crate::cmd_setup::SetupRuntimeScope::Detected => {
            runtimes.iter().filter(|runtime| runtime.detected).collect()
        }
        crate::cmd_setup::SetupRuntimeScope::All => runtimes.iter().collect(),
        crate::cmd_setup::SetupRuntimeScope::Single(slug) => runtimes
            .iter()
            .filter(|runtime| runtime.slug == slug)
            .collect(),
    }
}

pub(crate) fn push_prompt_targets(targets: &mut Vec<PromptTarget>, additions: &[PromptTarget]) {
    for addition in additions {
        if targets.iter().any(|target| target.path == addition.path) {
            continue;
        }
        targets.push(addition.clone());
    }
}

pub(crate) fn prompt_target_label(target: &PromptTarget) -> String {
    target
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| target.path.to_str().unwrap_or("prompt"))
        .to_string()
}
