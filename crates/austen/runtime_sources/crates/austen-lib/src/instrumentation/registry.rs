use super::Coverage;
use crate::EngineRegistry;
use crate::external::{DelegatedEngine, engine_and_token};

pub fn consumers(flavor: Coverage, registry: &EngineRegistry) -> Vec<&'static str> {
    registry
        .delegated_engines()
        .filter(|engine| DelegatedEngine::coverage(engine).contains(&flavor))
        .map(|engine| DelegatedEngine::name(&engine))
        .collect()
}

pub fn steered_by(flavor: Coverage, registry: &EngineRegistry) -> String {
    let consumers = registry
        .delegated_engines()
        .filter(|engine| DelegatedEngine::coverage(engine).contains(&flavor))
        .map(|engine| {
            engine_and_token(
                DelegatedEngine::display_name(&engine),
                DelegatedEngine::name(&engine),
            )
        })
        .collect::<Vec<_>>();
    if consumers.is_empty() {
        if flavor != Coverage::AflInlineMap {
            return "no registered engine steers by it".to_string();
        }
        return "no engine here can link it — austen's runtime keeps that \
                symbol local and cannot export it"
            .to_string();
    }
    format!(
        "only {} steer{} by it",
        consumers.join(" or "),
        if consumers.len() == 1 { "s" } else { "" }
    )
}
