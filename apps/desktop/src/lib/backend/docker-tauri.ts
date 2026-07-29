import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  DockerConnectionInfo,
  DockerContainer,
  DockerContainerAction,
  DockerContainerStats,
  DockerCreateContainerRequest,
  DockerCreateContainerResult,
  DockerCreateNetworkRequest,
  DockerCreateNetworkResult,
  DockerCreateVolumeRequest,
  DockerFileEntry,
  DockerFilePreview,
  DockerImage,
  DockerLogOptions,
  DockerNetwork,
  DockerRegistryAuth,
  DockerStreamEvent,
  DockerStreamHandle,
  DockerVolume,
} from "@/types/docker";

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

export function dockerCreateContainer(connectionId: string, request: DockerCreateContainerRequest): Promise<DockerCreateContainerResult> {
  return invoke("docker_create_container", { connectionId, request });
}

export function dockerRemoveContainer(connectionId: string, containerId: string): Promise<void> {
  return invoke("docker_remove_container", { connectionId, containerId });
}

export function dockerRemoveImage(connectionId: string, imageId: string): Promise<void> {
  return invoke("docker_remove_image", { connectionId, imageId });
}

export function dockerCreateVolume(connectionId: string, request: DockerCreateVolumeRequest): Promise<DockerVolume> {
  return invoke("docker_create_volume", { connectionId, request });
}

export function dockerCreateNetwork(connectionId: string, request: DockerCreateNetworkRequest): Promise<DockerCreateNetworkResult> {
  return invoke("docker_create_network", { connectionId, request });
}

export function dockerListContainerFiles(connectionId: string, containerId: string, path: string): Promise<DockerFileEntry[]> {
  return invoke("docker_list_container_files", { connectionId, containerId, path });
}

export function dockerPreviewContainerFile(connectionId: string, containerId: string, path: string): Promise<DockerFilePreview> {
  return invoke("docker_preview_container_file", { connectionId, containerId, path });
}

export async function dockerDownloadContainerFile(connectionId: string, containerId: string, path: string): Promise<Uint8Array> {
  return new Uint8Array(await invoke<number[]>("docker_download_container_file", { connectionId, containerId, path }));
}

export async function dockerExportImage(connectionId: string, imageId: string): Promise<Uint8Array> {
  return new Uint8Array(await invoke<number[]>("docker_export_image", { connectionId, imageId }));
}

function streamSessionId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `docker-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

async function startTauriStream(
  eventName: "docker-log-stream" | "docker-image-pull",
  command: "docker_start_logs" | "docker_pull_image",
  payload: Record<string, unknown>,
  onEvent: (event: DockerStreamEvent) => void,
): Promise<DockerStreamHandle> {
  const sessionId = streamSessionId();
  let stopped = false;
  const unlisten = await listen<DockerStreamEvent>(eventName, (event) => {
    if (event.payload.sessionId !== sessionId) return;
    onEvent(event.payload);
    if (event.payload.done) {
      stopped = true;
      unlisten();
    }
  });
  try {
    await invoke(command, { ...payload, sessionId });
  } catch (error) {
    unlisten();
    throw error;
  }
  return {
    sessionId,
    stop: async () => {
      if (stopped) return;
      stopped = true;
      unlisten();
      await invoke("docker_stop_stream", { sessionId });
    },
  };
}

export function dockerStartLogs(
  connectionId: string,
  containerId: string,
  options: DockerLogOptions,
  onEvent: (event: DockerStreamEvent) => void,
): Promise<DockerStreamHandle> {
  return startTauriStream("docker-log-stream", "docker_start_logs", { connectionId, containerId, options }, onEvent);
}

export function dockerPullImage(
  connectionId: string,
  image: string,
  auth: DockerRegistryAuth | undefined,
  onEvent: (event: DockerStreamEvent) => void,
): Promise<DockerStreamHandle> {
  return startTauriStream("docker-image-pull", "docker_pull_image", { connectionId, image, auth }, onEvent);
}
