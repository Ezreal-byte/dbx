use futures::{stream, StreamExt, TryStreamExt};
use serde_json::Value;

use crate::connection::AppState;
use crate::models::connection::{ConnectionConfig, DatabaseType};

use super::client::{encoded_id, DockerClient};
use super::config::DockerAdminConfig;
use super::types::*;

async fn connection_and_client(
    state: &AppState,
    connection_id: &str,
) -> Result<(ConnectionConfig, DockerClient, DockerVersionResponse), String> {
    let connection = state
        .configs
        .read()
        .await
        .get(connection_id)
        .cloned()
        .ok_or_else(|| "Docker connection not found".to_string())?;
    if connection.db_type != DatabaseType::Docker {
        return Err("Connection is not a Docker connection".to_string());
    }
    let config = DockerAdminConfig::from_connection(&connection)?;
    let (client, version) = DockerClient::connect(state, connection_id, &connection, &config).await?;
    Ok((connection, client, version))
}

pub async fn docker_test_connection_core(
    state: &AppState,
    connection_id: &str,
) -> Result<DockerConnectionInfo, String> {
    let (_, _, version) = connection_and_client(state, connection_id).await?;
    Ok(DockerConnectionInfo {
        engine_version: version.version,
        api_version: version.api_version,
        minimum_api_version: version.min_api_version,
        operating_system: version.os,
        architecture: version.arch,
    })
}

pub async fn docker_list_containers_core(
    state: &AppState,
    connection_id: &str,
    all: bool,
) -> Result<Vec<DockerContainer>, String> {
    let (_, client, _) = connection_and_client(state, connection_id).await?;
    let values: Vec<DockerContainerWire> =
        client.get(&format!("/containers/json?all={}", if all { 1 } else { 0 })).await?;
    Ok(values.into_iter().map(Into::into).collect())
}

pub async fn docker_list_images_core(state: &AppState, connection_id: &str) -> Result<Vec<DockerImage>, String> {
    let (_, client, _) = connection_and_client(state, connection_id).await?;
    let values: Vec<DockerImageWire> = client.get("/images/json?all=0").await?;
    Ok(values.into_iter().map(Into::into).collect())
}

pub async fn docker_list_volumes_core(state: &AppState, connection_id: &str) -> Result<Vec<DockerVolume>, String> {
    let (_, client, _) = connection_and_client(state, connection_id).await?;
    let value: DockerVolumeListWire = client.get("/volumes").await?;
    Ok(value.volumes.into_iter().map(Into::into).collect())
}

pub async fn docker_list_networks_core(state: &AppState, connection_id: &str) -> Result<Vec<DockerNetwork>, String> {
    let (_, client, _) = connection_and_client(state, connection_id).await?;
    let values: Vec<DockerNetworkWire> = client.get("/networks").await?;
    Ok(values.into_iter().map(Into::into).collect())
}

pub async fn docker_container_action_core(
    state: &AppState,
    connection_id: &str,
    container_id: &str,
    action: DockerContainerAction,
) -> Result<(), String> {
    let (connection, client, _) = connection_and_client(state, connection_id).await?;
    if connection.read_only {
        return Err("Docker connection is read-only; lifecycle operations are disabled".to_string());
    }
    let action_name = match action {
        DockerContainerAction::Start => "start",
        DockerContainerAction::Stop => "stop",
        DockerContainerAction::Restart => "restart",
    };
    let path = format!("/containers/{}/{action_name}", encoded_id(container_id));
    let result = client.post_empty(&path).await;
    match &result {
        Ok(()) => log::info!(
            "Docker lifecycle action succeeded: connection_id={} container_id={} action={}",
            connection_id,
            container_id,
            action_name
        ),
        Err(error) => log::warn!(
            "Docker lifecycle action failed: connection_id={} container_id={} action={} error={}",
            connection_id,
            container_id,
            action_name,
            error
        ),
    }
    result
}

pub async fn docker_inspect_container_core(
    state: &AppState,
    connection_id: &str,
    container_id: &str,
) -> Result<Value, String> {
    let (_, client, _) = connection_and_client(state, connection_id).await?;
    client.get_value(&format!("/containers/{}/json", encoded_id(container_id))).await
}

pub async fn docker_container_stats_core(
    state: &AppState,
    connection_id: &str,
    container_ids: Vec<String>,
) -> Result<Vec<DockerContainerStats>, String> {
    if container_ids.len() > 128 {
        return Err("At most 128 Docker containers can be sampled at once".to_string());
    }
    let (_, client, _) = connection_and_client(state, connection_id).await?;
    stream::iter(container_ids)
        .map(|container_id| {
            let client = &client;
            async move {
                let value =
                    client.get_value(&format!("/containers/{}/stats?stream=false", encoded_id(&container_id))).await?;
                Ok(stats_from_value(container_id, &value))
            }
        })
        .buffer_unordered(8)
        .try_collect()
        .await
}

fn stats_from_value(container_id: String, value: &Value) -> DockerContainerStats {
    let cpu_total = u64_at(value, &["cpu_stats", "cpu_usage", "total_usage"]);
    let previous_cpu_total = u64_at(value, &["precpu_stats", "cpu_usage", "total_usage"]);
    let system_total = u64_at(value, &["cpu_stats", "system_cpu_usage"]);
    let previous_system_total = u64_at(value, &["precpu_stats", "system_cpu_usage"]);
    let cpu_count = u64_at(value, &["cpu_stats", "online_cpus"])
        .max(value.pointer("/cpu_stats/cpu_usage/percpu_usage").and_then(Value::as_array).map_or(0, |v| v.len() as u64))
        .max(1);
    let cpu_delta = cpu_total.saturating_sub(previous_cpu_total);
    let system_delta = system_total.saturating_sub(previous_system_total);
    let cpu_percent =
        if system_delta == 0 { 0.0 } else { cpu_delta as f64 / system_delta as f64 * cpu_count as f64 * 100.0 };

    let raw_memory = u64_at(value, &["memory_stats", "usage"]);
    let cache = u64_at(value, &["memory_stats", "stats", "total_inactive_file"])
        .max(u64_at(value, &["memory_stats", "stats", "inactive_file"]))
        .max(u64_at(value, &["memory_stats", "stats", "cache"]));
    let memory_usage = raw_memory.saturating_sub(cache);
    let memory_limit = u64_at(value, &["memory_stats", "limit"]);
    let memory_percent = if memory_limit == 0 { 0.0 } else { memory_usage as f64 / memory_limit as f64 * 100.0 };

    let (network_rx, network_tx) = value
        .get("networks")
        .and_then(Value::as_object)
        .map(|networks| {
            networks.values().fold((0u64, 0u64), |(rx, tx), network| {
                (
                    rx.saturating_add(network.get("rx_bytes").and_then(Value::as_u64).unwrap_or_default()),
                    tx.saturating_add(network.get("tx_bytes").and_then(Value::as_u64).unwrap_or_default()),
                )
            })
        })
        .unwrap_or_default();
    let (block_read, block_write) = value
        .pointer("/blkio_stats/io_service_bytes_recursive")
        .and_then(Value::as_array)
        .map(|entries| {
            entries.iter().fold((0u64, 0u64), |(read, write), entry| {
                let amount = entry.get("value").and_then(Value::as_u64).unwrap_or_default();
                match entry.get("op").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase().as_str() {
                    "read" => (read.saturating_add(amount), write),
                    "write" => (read, write.saturating_add(amount)),
                    _ => (read, write),
                }
            })
        })
        .unwrap_or_default();

    DockerContainerStats {
        container_id,
        read_at: value.get("read").and_then(Value::as_str).unwrap_or_default().to_string(),
        cpu_percent,
        memory_usage,
        memory_limit,
        memory_percent,
        network_rx,
        network_tx,
        block_read,
        block_write,
    }
}

fn u64_at(value: &Value, path: &[&str]) -> u64 {
    path.iter().try_fold(value, |current, key| current.get(*key)).and_then(Value::as_u64).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::stats_from_value;

    #[test]
    fn calculates_stats_and_avoids_underflow() {
        let value = serde_json::json!({
            "read": "2026-07-29T12:00:00Z",
            "cpu_stats": {"cpu_usage": {"total_usage": 300}, "system_cpu_usage": 1000, "online_cpus": 2},
            "precpu_stats": {"cpu_usage": {"total_usage": 100}, "system_cpu_usage": 500},
            "memory_stats": {"usage": 1000, "limit": 2000, "stats": {"inactive_file": 250}},
            "networks": {"eth0": {"rx_bytes": 10, "tx_bytes": 20}},
            "blkio_stats": {"io_service_bytes_recursive": [{"op": "Read", "value": 30}, {"op": "Write", "value": 40}]}
        });
        let stats = stats_from_value("container".to_string(), &value);
        assert_eq!(stats.cpu_percent, 80.0);
        assert_eq!(stats.memory_usage, 750);
        assert_eq!(stats.memory_percent, 37.5);
        assert_eq!((stats.network_rx, stats.network_tx), (10, 20));
        assert_eq!((stats.block_read, stats.block_write), (30, 40));
    }
}
