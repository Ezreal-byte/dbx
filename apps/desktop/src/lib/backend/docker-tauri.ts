import { invoke } from "@tauri-apps/api/core";
import type { DockerConnectionInfo, DockerContainer, DockerContainerAction, DockerContainerStats, DockerImage, DockerNetwork, DockerVolume } from "@/types/docker";

export function dockerTestConnection(connectionId: string): Promise<DockerConnectionInfo> {
  return invoke("docker_test_connection", { connectionId });
}

export function dockerListContainers(connectionId: string, all = true): Promise<DockerContainer[]> {
  return invoke("docker_list_containers", { connectionId, all });
}

export function dockerListImages(connectionId: string): Promise<DockerImage[]> {
  return invoke("docker_list_images", { connectionId });
}

export function dockerListVolumes(connectionId: string): Promise<DockerVolume[]> {
  return invoke("docker_list_volumes", { connectionId });
}

export function dockerListNetworks(connectionId: string): Promise<DockerNetwork[]> {
  return invoke("docker_list_networks", { connectionId });
}

export function dockerContainerAction(connectionId: string, containerId: string, action: DockerContainerAction): Promise<void> {
  return invoke("docker_container_action", { connectionId, containerId, action });
}

export function dockerInspectContainer(connectionId: string, containerId: string): Promise<unknown> {
  return invoke("docker_inspect_container", { connectionId, containerId });
}

export function dockerContainerStats(connectionId: string, containerIds: string[]): Promise<DockerContainerStats[]> {
  return invoke("docker_container_stats", { connectionId, containerIds });
}
