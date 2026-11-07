use super::*;

pub(super) fn select<'a>(
    options: &'a [SessionConfigOption],
    id: &str,
) -> Result<&'a SessionConfigSelect> {
    let mut matches = options.iter().filter(|option| option.id.0.as_ref() == id);
    let option = matches.next().ok_or(Error("ACP_CONFIG_UNSUPPORTED"))?;
    if matches.next().is_some() {
        return Err(Error("ACP_CONFIG_AMBIGUOUS"));
    }
    match &option.kind {
        SessionConfigKind::Select(select) => Ok(select),
        _ => Err(Error("ACP_CONFIG_UNSUPPORTED")),
    }
}
pub(super) fn validate(options: &[SessionConfigOption], id: &str, value: &str) -> Result<()> {
    let select = select(options, id)?;
    let count = match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .filter(|option| option.value.0.as_ref() == value)
            .count(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| &group.options)
            .filter(|option| option.value.0.as_ref() == value)
            .count(),
        _ => 0,
    };
    if count != 1 {
        return Err(Error("ACP_CONFIG_VALUE_INVALID"));
    }
    Ok(())
}
