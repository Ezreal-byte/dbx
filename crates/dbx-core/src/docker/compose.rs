use std::collections::HashMap;

use serde_json::Value;

use crate::connection::AppState;

use super::{
    docker_container_action_core, docker_create_container_core, docker_create_network_core,
    docker_list_containers_core, docker_list_networks_core, docker_remove_container_core, DockerComposeApplyRequest,
    DockerComposeApplyResult, DockerContainerAction, DockerCreateContainerRequest, DockerCreateNetworkRequest,
    DockerMountInput, DockerPortBinding,
};

pub async fn docker_apply_compose_core(
    state: &AppState,
    connection_id: &str,
    request: DockerComposeApplyRequest,
) -> Result<DockerComposeApplyResult, String> {
    let project = validate_project_name(&request.project_name)?;
    let document: Value =
        serde_yaml_ng::from_str(&request.content).map_err(|error| format!("Invalid Compose YAML: {error}"))?;
    let services =
        document.get("services").and_then(Value::as_object).ok_or("Compose document must contain a services object")?;
    if services.is_empty() {
        return Err("Compose document must define at least one service".to_string());
    }

    if request.replace_existing {
        let existing = docker_list_containers_core(state, connection_id, true).await?;
        for container in existing
            .into_iter()
            .filter(|container| container.labels.get("com.docker.compose.project") == Some(&project))
        {
            if container.state.eq_ignore_ascii_case("paused") {
                docker_container_action_core(state, connection_id, &container.id, DockerContainerAction::Unpause)
                    .await?;
            }
            if container.state.eq_ignore_ascii_case("running") || container.state.eq_ignore_ascii_case("paused") {
                docker_container_action_core(state, connection_id, &container.id, DockerContainerAction::Stop).await?;
            }
            docker_remove_container_core(state, connection_id, &container.id).await?;
        }
    }

    let existing_networks = docker_list_networks_core(state, connection_id).await?;
    let mut network_names: Vec<String> = existing_networks.into_iter().map(|network| network.name).collect();
    let mut results = Vec::new();
    let mut warnings = Vec::new();

    for (index, (service_name, service)) in services.iter().enumerate() {
        let service = service.as_object().ok_or_else(|| format!("Service {service_name} must be an object"))?;
        let image = required_string(service.get("image"), &format!("Service {service_name} image"))?;
        let container_name = service
            .get("container_name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{project}-{service_name}-1"));
        let command = string_list(service.get("command"));
        let environment = environment_list(service.get("environment"));
        let ports = service
            .get("ports")
            .and_then(Value::as_array)
            .map(|values| values.iter().map(parse_port).collect::<Result<Vec<_>, _>>())
            .transpose()?
            .unwrap_or_default();
        let mounts = service
            .get("volumes")
            .and_then(Value::as_array)
            .map(|values| values.iter().map(|value| parse_mount(value, &project)).collect::<Result<Vec<_>, _>>())
            .transpose()?
            .unwrap_or_default();
        let requested_network = first_network(service.get("networks")).unwrap_or_else(|| "default".to_string());
        let network = if requested_network == "host" || requested_network == "none" {
            None
        } else {
            let resolved = format!("{project}_{requested_network}");
            if !network_names.contains(&resolved) {
                docker_create_network_core(
                    state,
                    connection_id,
                    DockerCreateNetworkRequest {
                        name: resolved.clone(),
                        driver: "bridge".to_string(),
                        internal: false,
                        attachable: false,
                        subnet: None,
                        gateway: None,
                    },
                )
                .await?;
                network_names.push(resolved.clone());
            }
            Some(resolved)
        };
        let mut labels = string_map(service.get("labels"));
        labels.insert("com.docker.compose.project".to_string(), project.clone());
        labels.insert("com.docker.compose.service".to_string(), service_name.clone());
        labels.insert("com.docker.compose.container-number".to_string(), "1".to_string());
        labels.insert("com.docker.compose.oneoff".to_string(), "False".to_string());
        let restart_policy = service
            .get("restart")
            .and_then(Value::as_str)
            .unwrap_or("no")
            .split(':')
            .next()
            .unwrap_or("no")
            .to_string();

        let result = docker_create_container_core(
            state,
            connection_id,
            DockerCreateContainerRequest {
                name: container_name,
                image,
                command,
                environment,
                ports,
                mounts,
                labels,
                network,
                restart_policy,
                start: true,
            },
        )
        .await
        .map_err(|error| format!("Failed to create Compose service {service_name}: {error}"))?;
        results.push(result.id);
        warnings.extend(result.warnings);
        if index == 0 && services.len() > 1 {
            warnings.push(
                "Services are created in document order; depends_on health conditions are not evaluated.".to_string(),
            );
        }
    }

    Ok(DockerComposeApplyResult { container_ids: results, warnings })
}

fn validate_project_name(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 63
        || !value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("Compose project name must contain only letters, numbers, hyphens, or underscores".to_string());
    }
    Ok(value)
}

fn required_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{field} is required"))
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(value_string).collect(),
        Some(Value::String(value)) => value.split_whitespace().map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn environment_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(value_string).collect(),
        Some(Value::Object(values)) => {
            values.iter().map(|(key, value)| format!("{key}={}", value_string(value).unwrap_or_default())).collect()
        }
        _ => Vec::new(),
    }
}

fn string_map(value: Option<&Value>) -> HashMap<String, String> {
    match value {
        Some(Value::Object(values)) => {
            values.iter().map(|(key, value)| (key.clone(), value_string(value).unwrap_or_default())).collect()
        }
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(value_string)
            .filter_map(|value| value.split_once('=').map(|(key, value)| (key.to_string(), value.to_string())))
            .collect(),
        _ => HashMap::new(),
    }
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

fn first_network(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Array(values)) => values.first().and_then(value_string),
        Some(Value::Object(values)) => values.keys().next().cloned(),
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn parse_port(value: &Value) -> Result<DockerPortBinding, String> {
    let text = value_string(value).ok_or("Compose ports must use short string syntax")?;
    let (mapping, protocol) = text.rsplit_once('/').map_or((text.as_str(), "tcp"), |parts| parts);
    let mut parts = mapping.rsplitn(3, ':');
    let container_port = parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| format!("Invalid Compose port mapping: {text}"))?;
    let host_port = parts.next().and_then(|value| value.parse::<u16>().ok());
    let host_ip = parts.next().unwrap_or_default().to_string();
    Ok(DockerPortBinding { container_port, protocol: protocol.to_string(), host_ip, host_port })
}

fn parse_mount(value: &Value, project: &str) -> Result<DockerMountInput, String> {
    let text = value_string(value).ok_or("Compose volumes must use short string syntax")?;
    let mut parts = text.rsplitn(3, ':');
    let mode_or_target = parts.next().unwrap_or_default();
    let (target, read_only) = if matches!(mode_or_target, "ro" | "rw") {
        (parts.next().unwrap_or_default(), mode_or_target == "ro")
    } else {
        (mode_or_target, false)
    };
    let source = parts.next().ok_or_else(|| format!("Invalid Compose volume mapping: {text}"))?;
    let is_bind = source.starts_with('/') || source.starts_with('.') || source.as_bytes().get(1) == Some(&b':');
    Ok(DockerMountInput {
        mount_type: if is_bind { "bind" } else { "volume" }.to_string(),
        source: if is_bind { source.to_string() } else { format!("{project}_{source}") },
        target: target.to_string(),
        read_only,
    })
}
