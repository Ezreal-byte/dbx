export type DockerProtocol = "http" | "https" | "unix" | "unix-over-nc" | "unix-over-nc-sudo";

export interface DockerAdminConfig {
  protocol: DockerProtocol;
  socketPath?: string;
  apiVersion?: "auto" | string;
  allowInsecureRemoteHttp?: boolean;
}

export interface DockerConnectionInfo {
  engineVersion: string;
  apiVersion: string;
  minimumApiVersion?: string | null;
  operatingSystem?: string | null;
  architecture?: string | null;
}

export interface DockerPort {
  ip?: string | null;
  privatePort: number;
  publicPort?: number | null;
  portType: string;
}

export interface DockerContainer {
  id: string;
  names: string[];
  image: string;
  imageId: string;
  command: string;
  created: number;
  state: string;
  status: string;
  ports: DockerPort[];
  labels: Record<string, string>;
  networkIps: Record<string, string>;
}

export interface DockerImage {
  id: string;
  repoTags: string[];
  repoDigests: string[];
  created: number;
  size: number;
  labels: Record<string, string>;
}

export interface DockerVolume {
  name: string;
  driver: string;
  mountpoint: string;
  scope: string;
  labels: Record<string, string>;
}

export interface DockerNetwork {
  id: string;
  name: string;
  driver: string;
  scope: string;
  internal: boolean;
  attachable: boolean;
  labels: Record<string, string>;
}

export type DockerContainerAction = "start" | "stop" | "restart";

export interface DockerContainerStats {
  containerId: string;
  readAt: string;
  cpuPercent: number;
  memoryUsage: number;
  memoryLimit: number;
  memoryPercent: number;
  networkRx: number;
  networkTx: number;
  blockRead: number;
  blockWrite: number;
}
