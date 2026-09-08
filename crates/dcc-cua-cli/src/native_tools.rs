use super::*;

pub(super) async fn list_apps(
    driver: &ComputerUseDriver,
) -> Result<(), Box<dyn std::error::Error>> {
    stdoutln!(
        "{}",
        serde_json::to_string_pretty(&driver.list_apps().await?)?
    );
    Ok(())
}

pub(super) async fn list_tools(
    driver: &ComputerUseDriver,
) -> Result<(), Box<dyn std::error::Error>> {
    stdoutln!(
        "{}",
        serde_json::to_string_pretty(&driver.list_tools().await?)?
    );
    Ok(())
}

pub(super) async fn call_tool(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let name = flag_value(flags, "--tool").ok_or("call requires --tool NAME")?;
    let arguments = json_arguments(flags)?;
    let output = flag_value(flags, "--output");
    let has_target = ["--app", "--pid", "--window-id", "--title"]
        .into_iter()
        .any(|flag| flag_value(flags, flag).is_some());
    if has_target && let Some(action) = actions::action_from_tool_call(&name, &arguments)? {
        return actions::execute_tool_action(driver, flags, action).await;
    }
    let result = if has_target {
        let scope = select_scope(driver, flags).await?;
        let app = application_label(flags);
        let session_id =
            flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-call-cli".into());
        let mut session = driver.session(scope, app, session_id)?;
        session.start().await?;
        let result = session.call_tool(&name, arguments).await;
        let stop_result = session.stop().await;
        let result = result?;
        stop_result?;
        result
    } else {
        driver.call_global_tool(&name, arguments).await?
    };
    if let (Some(path), Some(image)) = (output.as_deref(), result.images.first()) {
        fs::write(path, &image.data)?;
    }
    let mut value = result.value;
    if let Some(path) = output {
        value["_dcc_cua_image_output"] = json!(path);
    }
    stdoutln!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
