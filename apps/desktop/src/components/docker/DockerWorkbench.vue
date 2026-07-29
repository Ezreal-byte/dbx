<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  ArrowLeft,
  Box,
  ChevronDown,
  ChevronRight,
  CirclePause,
  CircleStop,
  Download,
  File,
  Folder,
  Pause,
  Play,
  Plus,
  RefreshCw,
  RotateCw,
  Search,
  Trash2,
  Upload,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import MetricLineChart from "@/components/chart/MetricLineChart.vue";
import DatabaseIcon from "@/components/icons/DatabaseIcon.vue";
import { useToast } from "@/composables/useToast";
import { hexToRgba } from "@/lib/common/color";
import * as api from "@/lib/backend/api";
import type { ConnectionConfig } from "@/types/database";
import type {
  DockerConnectionInfo,
  DockerContainer,
  DockerContainerAction,
  DockerContainerStats,
  DockerCreateContainerRequest,
  DockerCreateNetworkRequest,
  DockerCreateVolumeRequest,
  DockerFileEntry,
  DockerFilePreview,
  DockerImage,
  DockerNetwork,
  DockerRegistryAuth,
  DockerStreamHandle,
  DockerVolume,
} from "@/types/docker";

const props = defineProps<{ connection: ConnectionConfig }>();
const { t } = useI18n();
const { toast } = useToast();

type ResourceKind = "containers" | "images" | "volumes" | "networks";
type ContainerFilter = "all" | "running" | "stopped";
type DetailTab = "overview" | "logs" | "monitoring" | "files";
type TrendPoint = DockerContainerStats;

const resource = ref<ResourceKind>("containers");
const filter = ref<ContainerFilter>("all");
const loading = ref(false);
const error = ref("");
const query = ref("");
const engineInfo = ref<DockerConnectionInfo>();
const containers = ref<DockerContainer[]>([]);
const images = ref<DockerImage[]>([]);
const volumes = ref<DockerVolume[]>([]);
const networks = ref<DockerNetwork[]>([]);
const listStats = ref<Record<string, DockerContainerStats>>({});
const expandedProjects = ref(new Set<string>());
const selectedContainerId = ref("");
const detailTab = ref<DetailTab>("overview");
const inspect = ref<Record<string, any>>({});
const trend = ref<TrendPoint[]>([]);
const actionInFlight = ref<Record<string, string | undefined>>({});

const createContainerOpen = ref(false);
const pullImageOpen = ref(false);
const createVolumeOpen = ref(false);
const createNetworkOpen = ref(false);
const submitting = ref(false);
const pullProgress = ref("");
const createContainerDraft = ref({
  name: "",
  image: "",
  command: "",
  environment: "",
  ports: "",
  mounts: "",
  network: "",
  restartPolicy: "no",
  start: true,
});
const pullDraft = ref({ image: "", serverAddress: "", username: "", password: "" });
const volumeDraft = ref({ name: "", driver: "local", labels: "", driverOptions: "" });
const networkDraft = ref({ name: "", driver: "bridge", internal: false, attachable: false, subnet: "", gateway: "" });

const logText = ref("");
const pendingLogText = ref("");
const logPaused = ref(false);
const logSearch = ref("");
const logStream = ref<DockerStreamHandle>();
const logError = ref("");
const filePath = ref("/");
const fileEntries = ref<DockerFileEntry[]>([]);
const filePreview = ref<DockerFilePreview>();
const fileLoading = ref(false);
const fileError = ref("");
let listStatsTimer: number | undefined;
let detailStatsTimer: number | undefined;

const selectedContainer = computed(() => containers.value.find((container) => container.id === selectedContainerId.value));
const normalizedQuery = computed(() => query.value.trim().toLowerCase());
const isReadOnly = computed(() => !!props.connection.read_only);
const workbenchStyle = computed(() => {
  if (!props.connection.color) return undefined;
  return {
    "--docker-accent": props.connection.color,
    "--docker-accent-soft": hexToRgba(props.connection.color, 0.1),
  };
});

function containerName(container: DockerContainer): string {
  return container.labels["com.docker.compose.container-number"] && container.labels["com.docker.compose.service"]
    ? container.labels["com.docker.compose.service"]
    : container.names[0]?.replace(/^\//, "") || container.id.slice(0, 12);
}

function shortId(id: string): string {
  return id.replace(/^sha256:/, "").slice(0, 12);
}

function isRunning(container: DockerContainer): boolean {
  return container.state.toLowerCase() === "running";
}

function isPaused(container: DockerContainer): boolean {
  return container.state.toLowerCase() === "paused";
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`;
}

function formatPorts(container: DockerContainer): string {
  return container.ports
    .map((port) => `${port.ip || ""}${port.publicPort ? `:${port.publicPort}→` : ""}${port.privatePort}/${port.portType}`)
    .join(", ") || "—";
}

function formatDate(timestamp: number): string {
  return timestamp > 0 ? new Date(timestamp * 1000).toLocaleString() : "—";
}

const matchingContainers = computed(() =>
  containers.value.filter((container) => {
    if (filter.value === "running" && !isRunning(container) && !isPaused(container)) return false;
    if (filter.value === "stopped" && (isRunning(container) || isPaused(container))) return false;
    if (!normalizedQuery.value) return true;
    return [container.id, container.image, container.state, container.status, ...container.names, ...Object.values(container.labels)]
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery.value);
  }),
);

const composeGroups = computed(() => {
  const groups = new Map<string, DockerContainer[]>();
  for (const container of matchingContainers.value) {
    const project = container.labels["com.docker.compose.project"];
    if (!project) continue;
    const values = groups.get(project) ?? [];
    values.push(container);
    groups.set(project, values);
  }
  return [...groups.entries()].sort(([left], [right]) => left.localeCompare(right));
});

const standaloneContainers = computed(() =>
  matchingContainers.value.filter((container) => !container.labels["com.docker.compose.project"]),
);

const filteredImages = computed(() =>
  images.value.filter((item) =>
    !normalizedQuery.value
      ? true
      : [item.id, ...item.repoTags, ...item.repoDigests].join(" ").toLowerCase().includes(normalizedQuery.value),
  ),
);
const filteredVolumes = computed(() =>
  volumes.value.filter((item) => !normalizedQuery.value || [item.name, item.driver, item.mountpoint].join(" ").toLowerCase().includes(normalizedQuery.value)),
);
const filteredNetworks = computed(() =>
  networks.value.filter((item) => !normalizedQuery.value || [item.id, item.name, item.driver].join(" ").toLowerCase().includes(normalizedQuery.value)),
);
const visibleLogs = computed(() => {
  if (!logSearch.value.trim()) return logText.value;
  const needle = logSearch.value.toLowerCase();
  return logText.value
    .split("\n")
    .filter((line) => line.toLowerCase().includes(needle))
    .join("\n");
});
const trendLabels = computed(() => trend.value.map((point) => new Date(point.readAt || Date.now()).toLocaleTimeString()));
const cpuSeries = computed(() => [{ name: "CPU", data: trend.value.map((point) => point.cpuPercent), color: "#3b82f6" }]);
const memorySeries = computed(() => [{ name: t("docker.memory"), data: trend.value.map((point) => point.memoryPercent), color: "#8b5cf6" }]);

async function loadEngineInfo() {
  try {
    engineInfo.value = await api.dockerTestConnection(props.connection.id);
  } catch (cause: any) {
    error.value = cause?.message || String(cause);
  }
}

async function loadContainers() {
  containers.value = await api.dockerListContainers(props.connection.id, true);
  for (const [project] of composeGroups.value) expandedProjects.value.add(project);
}

async function loadResource(kind = resource.value) {
  loading.value = true;
  error.value = "";
  try {
    if (kind === "containers") await loadContainers();
    if (kind === "images") images.value = await api.dockerListImages(props.connection.id);
    if (kind === "volumes") volumes.value = await api.dockerListVolumes(props.connection.id);
    if (kind === "networks") networks.value = await api.dockerListNetworks(props.connection.id);
  } catch (cause: any) {
    error.value = cause?.message || String(cause);
  } finally {
    loading.value = false;
  }
}

async function selectResource(kind: ResourceKind) {
  await closeDetail();
  resource.value = kind;
  query.value = "";
  await loadResource(kind);
}

function toggleProject(project: string) {
  const next = new Set(expandedProjects.value);
  next.has(project) ? next.delete(project) : next.add(project);
  expandedProjects.value = next;
}

async function openDetail(container: DockerContainer) {
  selectedContainerId.value = container.id;
  detailTab.value = "overview";
  inspect.value = (await api.dockerInspectContainer(props.connection.id, container.id)) as Record<string, any>;
  trend.value = [];
  restartDetailSampling();
}

async function closeDetail() {
  stopDetailSampling();
  await stopLogs();
  selectedContainerId.value = "";
  inspect.value = {};
  fileEntries.value = [];
  filePreview.value = undefined;
}

function confirmAction(container: DockerContainer, action: DockerContainerAction | "remove"): boolean {
  const dangerous = props.connection.is_production || ["stop", "restart", "remove"].includes(action);
  if (!dangerous) return true;
  return window.confirm(t("docker.confirmAction", { action: t(`docker.action.${action}`), name: containerName(container) }));
}

function confirmProductionMutation(action: string): boolean {
  return !props.connection.is_production || window.confirm(t("docker.confirmProductionMutation", { action }));
}

async function runAction(container: DockerContainer, action: DockerContainerAction) {
  if (isReadOnly.value || actionInFlight.value[container.id]) {
    if (isReadOnly.value) toast(t("docker.readOnly"), 2400);
    return;
  }
  if (!confirmAction(container, action)) return;
  actionInFlight.value = { ...actionInFlight.value, [container.id]: action };
  try {
    await api.dockerContainerAction(props.connection.id, container.id, action);
    toast(t("docker.actionSucceeded", { action: t(`docker.action.${action}`), name: containerName(container) }), 2400);
    await loadContainers();
    if (selectedContainerId.value === container.id) {
      inspect.value = (await api.dockerInspectContainer(props.connection.id, container.id)) as Record<string, any>;
    }
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    actionInFlight.value = { ...actionInFlight.value, [container.id]: undefined };
  }
}

async function removeContainer(container: DockerContainer) {
  if (isReadOnly.value || actionInFlight.value[container.id]) return;
  if (!confirmAction(container, "remove")) return;
  actionInFlight.value = { ...actionInFlight.value, [container.id]: "remove" };
  try {
    await api.dockerRemoveContainer(props.connection.id, container.id);
    toast(t("docker.containerRemoved", { name: containerName(container) }), 2400);
    await loadContainers();
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    actionInFlight.value = { ...actionInFlight.value, [container.id]: undefined };
  }
}

function parseKeyValues(text: string): Record<string, string> {
  return Object.fromEntries(
    text
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const index = line.indexOf("=");
        return index < 0 ? [line, ""] : [line.slice(0, index).trim(), line.slice(index + 1).trim()];
      }),
  );
}

function createContainerRequest(): DockerCreateContainerRequest {
  const ports = createContainerDraft.value.ports
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [mapping, protocol = "tcp"] = line.split("/");
      const parts = mapping.split(":");
      const containerPort = Number(parts.pop());
      const hostPortText = parts.pop();
      const hostIp = parts.join(":");
      return {
        containerPort,
        protocol: protocol.toLowerCase() === "udp" ? ("udp" as const) : ("tcp" as const),
        hostIp,
        hostPort: hostPortText ? Number(hostPortText) : undefined,
      };
    });
  const mounts = createContainerDraft.value.mounts
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const parts = line.split(":");
      const readOnly = parts[parts.length - 1] === "ro";
      if (readOnly) parts.pop();
      const source = parts.shift() || "";
      const target = parts.join(":");
      return { type: source.startsWith("/") || /^[A-Za-z]:[\\/]/.test(source) ? ("bind" as const) : ("volume" as const), source, target, readOnly };
    });
  return {
    name: createContainerDraft.value.name.trim(),
    image: createContainerDraft.value.image.trim(),
    command: createContainerDraft.value.command.split(/\r?\n/).map((value) => value.trim()).filter(Boolean),
    environment: createContainerDraft.value.environment.split(/\r?\n/).map((value) => value.trim()).filter(Boolean),
    ports,
    mounts,
    network: createContainerDraft.value.network || undefined,
    restartPolicy: createContainerDraft.value.restartPolicy as DockerCreateContainerRequest["restartPolicy"],
    start: createContainerDraft.value.start,
  };
}

async function createContainer() {
  if (!confirmProductionMutation(t("docker.createContainer"))) return;
  submitting.value = true;
  try {
    await api.dockerCreateContainer(props.connection.id, createContainerRequest());
    createContainerOpen.value = false;
    toast(t("docker.containerCreated"), 2400);
    await loadContainers();
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    submitting.value = false;
  }
}

async function openCreateContainer() {
  if (!networks.value.length) {
    try {
      networks.value = await api.dockerListNetworks(props.connection.id);
    } catch {
      // The form remains usable with Docker's default network.
    }
  }
  createContainerOpen.value = true;
}

async function pullImage() {
  if (!confirmProductionMutation(t("docker.pullImage"))) return;
  submitting.value = true;
  pullProgress.value = "";
  try {
    const auth: DockerRegistryAuth | undefined =
      pullDraft.value.serverAddress || pullDraft.value.username || pullDraft.value.password
        ? {
            serverAddress: pullDraft.value.serverAddress,
            username: pullDraft.value.username,
            password: pullDraft.value.password,
          }
        : undefined;
    await api.dockerPullImage(props.connection.id, pullDraft.value.image.trim(), auth, (event) => {
      if (event.chunk) pullProgress.value = `${pullProgress.value}${event.chunk}`.slice(-20_000);
      if (event.error) toast(event.error, 5000);
      if (event.done && !event.error) {
        toast(t("docker.imagePulled"), 2400);
        pullImageOpen.value = false;
        void loadResource("images");
      }
    });
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    submitting.value = false;
  }
}

function downloadBytes(bytes: Uint8Array | string, fileName: string, type = "application/octet-stream") {
  const blob = new Blob([typeof bytes === "string" ? bytes : bytes.slice().buffer], { type });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.click();
  URL.revokeObjectURL(url);
}

async function exportImage(item: DockerImage) {
  try {
    const bytes = await api.dockerExportImage(props.connection.id, item.id);
    downloadBytes(bytes, `${shortId(item.id)}.tar`);
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  }
}

async function removeImage(item: DockerImage) {
  if (!window.confirm(t("docker.confirmImageRemove", { name: item.repoTags[0] || shortId(item.id) }))) return;
  try {
    await api.dockerRemoveImage(props.connection.id, item.id);
    toast(t("docker.imageRemoved"), 2400);
    await loadResource("images");
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  }
}

async function createVolume() {
  if (!confirmProductionMutation(t("docker.createVolume"))) return;
  submitting.value = true;
  try {
    const request: DockerCreateVolumeRequest = {
      name: volumeDraft.value.name,
      driver: volumeDraft.value.driver || "local",
      labels: parseKeyValues(volumeDraft.value.labels),
      driverOptions: parseKeyValues(volumeDraft.value.driverOptions),
    };
    await api.dockerCreateVolume(props.connection.id, request);
    createVolumeOpen.value = false;
    toast(t("docker.volumeCreated"), 2400);
    await loadResource("volumes");
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    submitting.value = false;
  }
}

async function createNetwork() {
  if (!confirmProductionMutation(t("docker.createNetwork"))) return;
  submitting.value = true;
  try {
    const request: DockerCreateNetworkRequest = {
      name: networkDraft.value.name,
      driver: networkDraft.value.driver || "bridge",
      internal: networkDraft.value.internal,
      attachable: networkDraft.value.attachable,
      subnet: networkDraft.value.subnet || undefined,
      gateway: networkDraft.value.gateway || undefined,
    };
    await api.dockerCreateNetwork(props.connection.id, request);
    createNetworkOpen.value = false;
    toast(t("docker.networkCreated"), 2400);
    await loadResource("networks");
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    submitting.value = false;
  }
}

function appendLogs(chunk: string) {
  if (logPaused.value) {
    pendingLogText.value += chunk;
    if (pendingLogText.value.length > 5 * 1024 * 1024) pendingLogText.value = pendingLogText.value.slice(-5 * 1024 * 1024);
    return;
  }
  logText.value += chunk;
  const lines = logText.value.split("\n");
  if (lines.length > 10_000) logText.value = lines.slice(-10_000).join("\n");
  if (logText.value.length > 5 * 1024 * 1024) logText.value = logText.value.slice(-5 * 1024 * 1024);
}

async function startLogs() {
  if (!selectedContainer.value || logStream.value) return;
  logError.value = "";
  try {
    logStream.value = await api.dockerStartLogs(props.connection.id, selectedContainer.value.id, { tail: 500, timestamps: false }, (event) => {
      if (event.chunk) appendLogs(event.chunk);
      if (event.error) logError.value = event.error;
      if (event.done) logStream.value = undefined;
    });
  } catch (cause: any) {
    logError.value = cause?.message || String(cause);
  }
}

async function stopLogs() {
  const stream = logStream.value;
  logStream.value = undefined;
  if (stream) await stream.stop().catch(() => undefined);
}

function toggleLogPause() {
  logPaused.value = !logPaused.value;
  if (!logPaused.value && pendingLogText.value) {
    const pending = pendingLogText.value;
    pendingLogText.value = "";
    appendLogs(pending);
  }
}

function clearLogs() {
  logText.value = "";
  pendingLogText.value = "";
}

async function loadFiles(path = filePath.value) {
  if (!selectedContainer.value) return;
  fileLoading.value = true;
  fileError.value = "";
  filePreview.value = undefined;
  try {
    filePath.value = path;
    fileEntries.value = await api.dockerListContainerFiles(props.connection.id, selectedContainer.value.id, path);
  } catch (cause: any) {
    fileError.value = cause?.message || String(cause);
  } finally {
    fileLoading.value = false;
  }
}

function parentPath(path: string): string {
  if (path === "/") return "/";
  const result = path.replace(/\/+$/, "").replace(/\/[^/]+$/, "");
  return result || "/";
}

async function openFile(entry: DockerFileEntry) {
  if (!selectedContainer.value) return;
  if (entry.kind === "directory") {
    await loadFiles(entry.path);
    return;
  }
  fileLoading.value = true;
  try {
    filePreview.value = await api.dockerPreviewContainerFile(props.connection.id, selectedContainer.value.id, entry.path);
  } catch (cause: any) {
    fileError.value = cause?.message || String(cause);
  } finally {
    fileLoading.value = false;
  }
}

async function downloadFile(entry: DockerFileEntry) {
  if (!selectedContainer.value) return;
  try {
    const bytes = await api.dockerDownloadContainerFile(props.connection.id, selectedContainer.value.id, entry.path);
    downloadBytes(bytes, entry.name);
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  }
}

async function sampleVisibleContainers() {
  if (document.hidden || resource.value !== "containers" || selectedContainer.value) return;
  const ids = matchingContainers.value.filter(isRunning).map((container) => container.id);
  if (!ids.length) {
    listStats.value = {};
    return;
  }
  try {
    const stats = await api.dockerContainerStats(props.connection.id, ids);
    listStats.value = Object.fromEntries(stats.map((value) => [value.containerId, value]));
  } catch {
    // The resource refresh continues to provide state if metrics are unavailable.
  }
}

async function sampleSelectedContainer() {
  const container = selectedContainer.value;
  if (!container || !isRunning(container) || document.hidden || detailTab.value !== "monitoring") return;
  try {
    const [point] = await api.dockerContainerStats(props.connection.id, [container.id]);
    if (!point) return;
    const cutoff = Date.now() - 15 * 60 * 1000;
    trend.value = [...trend.value, point].filter((value) => new Date(value.readAt || Date.now()).getTime() >= cutoff);
  } catch {
    // Keep the last successful samples visible.
  }
}

function restartListSampling() {
  if (listStatsTimer) window.clearInterval(listStatsTimer);
  listStatsTimer = window.setInterval(() => void sampleVisibleContainers(), 5000);
  void sampleVisibleContainers();
}

function stopDetailSampling() {
  if (detailStatsTimer) window.clearInterval(detailStatsTimer);
  detailStatsTimer = undefined;
}

function restartDetailSampling() {
  stopDetailSampling();
  detailStatsTimer = window.setInterval(() => void sampleSelectedContainer(), 2000);
  void sampleSelectedContainer();
}

watch(detailTab, async (tab) => {
  if (tab === "logs") await startLogs();
  else await stopLogs();
  if (tab === "files" && !fileEntries.value.length) await loadFiles("/");
  restartDetailSampling();
});

watch(resource, restartListSampling);

onMounted(async () => {
  document.addEventListener("visibilitychange", restartDetailSampling);
  await Promise.all([loadEngineInfo(), loadResource("containers")]);
  restartListSampling();
});

onUnmounted(() => {
  document.removeEventListener("visibilitychange", restartDetailSampling);
  if (listStatsTimer) window.clearInterval(listStatsTimer);
  stopDetailSampling();
  void stopLogs();
});
</script>

<template>
  <div class="flex h-full min-h-0 flex-col bg-background text-foreground" :style="workbenchStyle">
    <header class="docker-header flex h-14 shrink-0 items-center border-b px-4">
      <div class="flex min-w-0 items-center gap-2.5">
        <DatabaseIcon db-type="docker" class="h-7 w-7 shrink-0" />
        <div class="min-w-0">
          <div class="truncate text-sm font-semibold">{{ connection.name }}</div>
          <div class="text-[11px] text-muted-foreground">Docker Workbench</div>
        </div>
      </div>
      <nav class="ml-8 flex h-full items-end gap-1">
        <button
          v-for="kind in ['containers', 'images', 'volumes', 'networks'] as ResourceKind[]"
          :key="kind"
          class="docker-main-tab"
          :class="{ active: resource === kind }"
          @click="selectResource(kind)"
        >
          {{ t(`docker.${kind}`) }}
        </button>
      </nav>
      <div class="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
        <span v-if="engineInfo">Engine {{ engineInfo.engineVersion }} · API {{ engineInfo.apiVersion }}</span>
        <span class="h-2.5 w-2.5 rounded-full" :class="engineInfo ? 'bg-emerald-500' : 'bg-destructive'" />
        <span>{{ engineInfo ? t("docker.connected") : t("docker.disconnected") }}</span>
      </div>
    </header>

    <div v-if="error" class="m-3 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">{{ error }}</div>

    <main v-else class="flex min-h-0 flex-1 flex-col">
      <template v-if="selectedContainer">
        <div class="flex h-12 shrink-0 items-center gap-3 border-b px-4">
          <Button size="sm" variant="ghost" @click="closeDetail"><ArrowLeft />{{ t("docker.backToContainers") }}</Button>
          <span class="h-5 w-px bg-border" />
          <div class="min-w-0">
            <div class="truncate text-sm font-semibold">{{ containerName(selectedContainer) }}</div>
            <div class="font-mono text-[10px] text-muted-foreground">{{ shortId(selectedContainer.id) }}</div>
          </div>
        </div>
        <div class="flex shrink-0 border-b px-4">
          <button
            v-for="tab in ['overview', 'logs', 'monitoring', 'files'] as DetailTab[]"
            :key="tab"
            class="docker-detail-tab"
            :class="{ active: detailTab === tab }"
            @click="detailTab = tab"
          >
            {{ t(`docker.detail.${tab}`) }}
          </button>
        </div>
        <div class="min-h-0 flex-1 overflow-auto p-4">
          <div v-if="detailTab === 'overview'" class="space-y-4">
            <section class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              <div class="docker-card"><span>{{ t("docker.fullId") }}</span><strong class="break-all font-mono text-xs">{{ selectedContainer.id }}</strong></div>
              <div class="docker-card"><span>{{ t("docker.image") }}</span><strong>{{ selectedContainer.image }}</strong><small class="break-all font-mono">{{ selectedContainer.imageId }}</small></div>
              <div class="docker-card"><span>{{ t("docker.status") }}</span><strong>{{ selectedContainer.state }}</strong><small>{{ selectedContainer.status }}</small></div>
              <div class="docker-card"><span>{{ t("docker.health") }}</span><strong>{{ inspect.State?.Health?.Status || "—" }}</strong></div>
              <div class="docker-card"><span>{{ t("docker.command") }}</span><strong class="font-mono text-xs">{{ [inspect.Path, ...(inspect.Args || [])].filter(Boolean).join(" ") || selectedContainer.command }}</strong></div>
              <div class="docker-card"><span>{{ t("docker.created") }}</span><strong>{{ formatDate(selectedContainer.created) }}</strong></div>
            </section>
            <section>
              <h3 class="mb-2 text-sm font-semibold">{{ t("docker.environment") }}</h3>
              <pre class="docker-code">{{ (inspect.Config?.Env || []).join("\n") || "—" }}</pre>
            </section>
            <section>
              <h3 class="mb-2 text-sm font-semibold">{{ t("docker.mounts") }}</h3>
              <div class="space-y-1">
                <div v-for="mount in inspect.Mounts || []" :key="`${mount.Source}-${mount.Destination}`" class="docker-row-value">
                  {{ mount.Source }} → {{ mount.Destination }} <span class="text-muted-foreground">({{ mount.Mode || mount.Type }})</span>
                </div>
                <div v-if="!inspect.Mounts?.length" class="docker-row-value">—</div>
              </div>
            </section>
            <section>
              <h3 class="mb-2 text-sm font-semibold">{{ t("docker.networks") }}</h3>
              <div class="space-y-1">
                <div v-for="(network, name) in inspect.NetworkSettings?.Networks || {}" :key="String(name)" class="docker-row-value">
                  {{ name }} · {{ network.IPAddress || "—" }}
                </div>
              </div>
            </section>
          </div>

          <div v-else-if="detailTab === 'logs'" class="flex h-full min-h-[28rem] flex-col">
            <div class="mb-2 flex items-center gap-2">
              <div class="relative w-72">
                <Search class="pointer-events-none absolute left-2 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input v-model="logSearch" class="pl-8" :placeholder="t('docker.searchLogs')" />
              </div>
              <Button size="sm" variant="outline" @click="toggleLogPause"><Play v-if="logPaused" /><Pause v-else />{{ logPaused ? t("docker.resume") : t("docker.pause") }}</Button>
              <Button size="sm" variant="outline" @click="clearLogs">{{ t("docker.clear") }}</Button>
              <Button size="sm" variant="outline" @click="downloadBytes(logText, `${containerName(selectedContainer)}.log`, 'text/plain;charset=utf-8')"><Download />{{ t("docker.download") }}</Button>
              <span v-if="pendingLogText" class="text-xs text-amber-600">{{ t("docker.bufferedLogs") }}</span>
            </div>
            <div v-if="logError" class="mb-2 text-sm text-destructive">{{ logError }}</div>
            <pre class="min-h-0 flex-1 overflow-auto rounded-md bg-zinc-950 p-3 font-mono text-xs leading-5 text-zinc-100">{{ visibleLogs || t("docker.waitingForLogs") }}</pre>
          </div>

          <div v-else-if="detailTab === 'monitoring'" class="grid gap-3 xl:grid-cols-2">
            <MetricLineChart title="CPU %" :labels="trendLabels" :series="cpuSeries" :height="240" :value-formatter="(value) => `${value.toFixed(1)}%`" />
            <MetricLineChart :title="`${t('docker.memory')} %`" :labels="trendLabels" :series="memorySeries" :height="240" :value-formatter="(value) => `${value.toFixed(1)}%`" />
          </div>

          <div v-else class="grid h-full min-h-[28rem] grid-cols-[minmax(20rem,36%)_1fr] overflow-hidden rounded-md border">
            <div class="flex min-h-0 flex-col border-r">
              <div class="flex items-center gap-2 border-b p-2">
                <Button size="sm" variant="ghost" :disabled="filePath === '/'" @click="loadFiles(parentPath(filePath))"><ArrowLeft /></Button>
                <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ filePath }}</span>
                <Button size="sm" variant="ghost" :disabled="fileLoading" @click="loadFiles()"><RefreshCw :class="{ 'animate-spin': fileLoading }" /></Button>
              </div>
              <div v-if="fileError" class="p-3 text-sm text-destructive">{{ fileError }}</div>
              <div v-else class="min-h-0 flex-1 overflow-auto">
                <button v-for="entry in fileEntries" :key="entry.path" class="flex w-full items-center gap-2 border-b px-3 py-2 text-left text-sm hover:bg-muted/50" @dblclick="openFile(entry)">
                  <Folder v-if="entry.kind === 'directory'" class="h-4 w-4 text-amber-500" />
                  <File v-else class="h-4 w-4 text-sky-500" />
                  <span class="min-w-0 flex-1 truncate">{{ entry.name }}</span>
                  <span class="text-xs tabular-nums text-muted-foreground">{{ entry.kind === "directory" ? "" : formatBytes(entry.size) }}</span>
                  <Button v-if="entry.kind === 'file'" size="icon-sm" variant="ghost" @click.stop="downloadFile(entry)"><Download /></Button>
                </button>
              </div>
            </div>
            <div class="min-h-0 overflow-auto bg-muted/10 p-3">
              <div v-if="filePreview?.binary" class="text-sm text-muted-foreground">{{ t("docker.binaryPreviewUnsupported") }}</div>
              <pre v-else-if="filePreview" class="whitespace-pre-wrap break-all font-mono text-xs">{{ filePreview.content }}<template v-if="filePreview.truncated">…</template></pre>
              <div v-else class="flex h-full items-center justify-center text-sm text-muted-foreground">{{ t("docker.selectFile") }}</div>
            </div>
          </div>
        </div>
      </template>

      <template v-else>
        <div class="flex h-14 shrink-0 items-center gap-2 border-b px-4">
          <div class="relative w-72">
            <Search class="pointer-events-none absolute left-2 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input v-model="query" class="pl-8" :placeholder="t('docker.search')" />
          </div>
          <template v-if="resource === 'containers'">
            <Button :disabled="isReadOnly" @click="openCreateContainer"><Plus />{{ t("docker.createContainer") }}</Button>
            <div class="ml-2 flex rounded-md border bg-muted/20 p-0.5">
              <button v-for="value in ['all', 'running', 'stopped'] as ContainerFilter[]" :key="value" class="rounded px-3 py-1.5 text-xs" :class="filter === value ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'" @click="filter = value">
                {{ t(`docker.filter.${value}`) }}
              </button>
            </div>
          </template>
          <Button v-else-if="resource === 'images'" :disabled="isReadOnly" @click="pullImageOpen = true"><Download />{{ t("docker.pullImage") }}</Button>
          <Button v-else-if="resource === 'volumes'" :disabled="isReadOnly" @click="createVolumeOpen = true"><Plus />{{ t("docker.createVolume") }}</Button>
          <Button v-else :disabled="isReadOnly" @click="createNetworkOpen = true"><Plus />{{ t("docker.createNetwork") }}</Button>
          <Button class="ml-auto" variant="ghost" :disabled="loading" @click="loadResource()"><RefreshCw :class="{ 'animate-spin': loading }" />{{ t("docker.refresh") }}</Button>
        </div>

        <div class="min-h-0 flex-1 overflow-auto">
          <table v-if="resource === 'containers'" class="docker-table">
            <thead><tr><th>{{ t("docker.name") }}</th><th>{{ t("docker.image") }}</th><th>{{ t("docker.ports") }}</th><th>CPU</th><th>{{ t("docker.memory") }}</th><th class="text-right">{{ t("docker.actions") }}</th></tr></thead>
            <tbody>
              <template v-for="[project, values] in composeGroups" :key="project">
                <tr class="bg-muted/25 font-medium">
                  <td colspan="6">
                    <button class="flex items-center gap-2" @click="toggleProject(project)">
                      <ChevronDown v-if="expandedProjects.has(project)" class="h-4 w-4" /><ChevronRight v-else class="h-4 w-4" />
                      <Box class="h-4 w-4 text-sky-500" />{{ project }}<span class="rounded bg-muted px-1.5 text-xs text-muted-foreground">{{ values.length }}</span>
                    </button>
                  </td>
                </tr>
                <tr v-for="container in expandedProjects.has(project) ? values : []" :key="container.id">
                  <td><div class="flex items-center gap-2 pl-6"><span class="h-2.5 w-2.5 rounded-full" :class="isRunning(container) ? 'bg-emerald-500' : isPaused(container) ? 'bg-amber-500' : 'bg-zinc-400'" /><button class="font-medium hover:underline" @click="openDetail(container)">{{ containerName(container) }}</button></div></td>
                  <td class="max-w-64 truncate">{{ container.image }}</td><td class="max-w-72 truncate font-mono text-xs">{{ formatPorts(container) }}</td>
                  <td>{{ listStats[container.id] ? `${listStats[container.id].cpuPercent.toFixed(1)}%` : "—" }}</td>
                  <td>{{ listStats[container.id] ? `${formatBytes(listStats[container.id].memoryUsage)} / ${formatBytes(listStats[container.id].memoryLimit)}` : "—" }}</td>
                  <td><div class="flex justify-end gap-1"><Button v-if="isRunning(container)" size="icon-sm" variant="ghost" :title="t('docker.pause')" @click="runAction(container, 'pause')"><CirclePause /></Button><Button v-if="isPaused(container)" size="icon-sm" variant="ghost" :title="t('docker.resume')" @click="runAction(container, 'unpause')"><Play /></Button><Button v-if="isRunning(container) || isPaused(container)" size="icon-sm" variant="ghost" :title="t('docker.restart')" @click="runAction(container, 'restart')"><RotateCw /></Button><Button v-if="isRunning(container) || isPaused(container)" size="icon-sm" variant="ghost" :title="t('docker.stop')" @click="runAction(container, 'stop')"><CircleStop /></Button><Button v-if="!isRunning(container) && !isPaused(container)" size="icon-sm" variant="ghost" :title="t('docker.start')" @click="runAction(container, 'start')"><Play /></Button><Button v-if="!isRunning(container) && !isPaused(container) && !isReadOnly" size="icon-sm" variant="ghost" :title="t('docker.remove')" @click="removeContainer(container)"><Trash2 /></Button><Button size="sm" variant="ghost" @click="openDetail(container)">{{ t("docker.details") }}</Button></div></td>
                </tr>
              </template>
              <tr v-for="container in standaloneContainers" :key="container.id">
                <td><div class="flex items-center gap-2"><span class="h-2.5 w-2.5 rounded-full" :class="isRunning(container) ? 'bg-emerald-500' : isPaused(container) ? 'bg-amber-500' : 'bg-zinc-400'" /><button class="font-medium hover:underline" @click="openDetail(container)">{{ containerName(container) }}</button></div></td>
                <td class="max-w-64 truncate">{{ container.image }}</td><td class="max-w-72 truncate font-mono text-xs">{{ formatPorts(container) }}</td>
                <td>{{ listStats[container.id] ? `${listStats[container.id].cpuPercent.toFixed(1)}%` : "—" }}</td>
                <td>{{ listStats[container.id] ? `${formatBytes(listStats[container.id].memoryUsage)} / ${formatBytes(listStats[container.id].memoryLimit)}` : "—" }}</td>
                <td><div class="flex justify-end gap-1"><Button v-if="isRunning(container)" size="icon-sm" variant="ghost" @click="runAction(container, 'pause')"><CirclePause /></Button><Button v-if="isPaused(container)" size="icon-sm" variant="ghost" @click="runAction(container, 'unpause')"><Play /></Button><Button v-if="isRunning(container) || isPaused(container)" size="icon-sm" variant="ghost" @click="runAction(container, 'restart')"><RotateCw /></Button><Button v-if="isRunning(container) || isPaused(container)" size="icon-sm" variant="ghost" @click="runAction(container, 'stop')"><CircleStop /></Button><Button v-if="!isRunning(container) && !isPaused(container)" size="icon-sm" variant="ghost" @click="runAction(container, 'start')"><Play /></Button><Button v-if="!isRunning(container) && !isPaused(container) && !isReadOnly" size="icon-sm" variant="ghost" @click="removeContainer(container)"><Trash2 /></Button><Button size="sm" variant="ghost" @click="openDetail(container)">{{ t("docker.details") }}</Button></div></td>
              </tr>
            </tbody>
          </table>

          <table v-else-if="resource === 'images'" class="docker-table">
            <thead><tr><th>{{ t("docker.repositoryTag") }}</th><th>ID</th><th>{{ t("docker.size") }}</th><th>{{ t("docker.created") }}</th><th class="text-right">{{ t("docker.actions") }}</th></tr></thead>
            <tbody><tr v-for="item in filteredImages" :key="item.id"><td>{{ item.repoTags.join(", ") || "&lt;none&gt;" }}</td><td class="font-mono text-xs">{{ shortId(item.id) }}</td><td>{{ formatBytes(item.size) }}</td><td>{{ formatDate(item.created) }}</td><td><div class="flex justify-end gap-1"><Button size="sm" variant="ghost" @click="exportImage(item)"><Upload />{{ t("docker.export") }}</Button><Button size="icon-sm" variant="ghost" :disabled="isReadOnly" @click="removeImage(item)"><Trash2 /></Button></div></td></tr></tbody>
          </table>
          <table v-else-if="resource === 'volumes'" class="docker-table">
            <thead><tr><th>{{ t("docker.name") }}</th><th>Driver</th><th>Scope</th><th>{{ t("docker.mountpoint") }}</th></tr></thead>
            <tbody><tr v-for="item in filteredVolumes" :key="item.name"><td class="font-medium">{{ item.name }}</td><td>{{ item.driver }}</td><td>{{ item.scope }}</td><td class="font-mono text-xs">{{ item.mountpoint }}</td></tr></tbody>
          </table>
          <table v-else class="docker-table">
            <thead><tr><th>{{ t("docker.name") }}</th><th>ID</th><th>Driver</th><th>Scope</th><th>Internal</th><th>Attachable</th></tr></thead>
            <tbody><tr v-for="item in filteredNetworks" :key="item.id"><td class="font-medium">{{ item.name }}</td><td class="font-mono text-xs">{{ shortId(item.id) }}</td><td>{{ item.driver }}</td><td>{{ item.scope }}</td><td>{{ item.internal ? t("common.yes") : t("common.no") }}</td><td>{{ item.attachable ? t("common.yes") : t("common.no") }}</td></tr></tbody>
          </table>
        </div>
      </template>
    </main>

    <Dialog v-model:open="createContainerOpen">
      <DialogContent class="max-h-[88vh] max-w-3xl overflow-auto">
        <DialogHeader><DialogTitle>{{ t("docker.createContainer") }}</DialogTitle><DialogDescription>{{ t("docker.createContainerDescription") }}</DialogDescription></DialogHeader>
        <div class="grid gap-4 py-2 md:grid-cols-2">
          <label class="docker-field"><span>{{ t("docker.name") }}</span><Input v-model="createContainerDraft.name" /></label>
          <label class="docker-field"><span>{{ t("docker.image") }}</span><Input v-model="createContainerDraft.image" placeholder="nginx:latest" /></label>
          <label class="docker-field"><span>{{ t("docker.commandLines") }}</span><textarea v-model="createContainerDraft.command" rows="4" class="docker-textarea" /></label>
          <label class="docker-field"><span>{{ t("docker.environmentLines") }}</span><textarea v-model="createContainerDraft.environment" rows="4" class="docker-textarea" placeholder="KEY=value" /></label>
          <label class="docker-field"><span>{{ t("docker.portLines") }}</span><textarea v-model="createContainerDraft.ports" rows="4" class="docker-textarea" placeholder="127.0.0.1:8080:80/tcp" /></label>
          <label class="docker-field"><span>{{ t("docker.mountLines") }}</span><textarea v-model="createContainerDraft.mounts" rows="4" class="docker-textarea" placeholder="volume-name:/data:ro" /></label>
          <label class="docker-field"><span>{{ t("docker.network") }}</span><select v-model="createContainerDraft.network" class="docker-select"><option value="">{{ t("docker.defaultNetwork") }}</option><option v-for="item in networks" :key="item.id" :value="item.name">{{ item.name }}</option></select></label>
          <label class="docker-field"><span>{{ t("docker.restartPolicy") }}</span><select v-model="createContainerDraft.restartPolicy" class="docker-select"><option value="no">no</option><option value="always">always</option><option value="unless-stopped">unless-stopped</option><option value="on-failure">on-failure</option></select></label>
          <label class="flex items-center gap-2 text-sm"><input v-model="createContainerDraft.start" type="checkbox" />{{ t("docker.startAfterCreate") }}</label>
        </div>
        <DialogFooter><Button variant="outline" @click="createContainerOpen = false">{{ t("common.cancel") }}</Button><Button :disabled="submitting || !createContainerDraft.name.trim() || !createContainerDraft.image.trim()" @click="createContainer">{{ t("docker.create") }}</Button></DialogFooter>
      </DialogContent>
    </Dialog>

    <Dialog v-model:open="pullImageOpen">
      <DialogContent>
        <DialogHeader><DialogTitle>{{ t("docker.pullImage") }}</DialogTitle><DialogDescription>{{ t("docker.registryCredentialsTemporary") }}</DialogDescription></DialogHeader>
        <div class="space-y-3 py-2"><label class="docker-field"><span>{{ t("docker.image") }}</span><Input v-model="pullDraft.image" placeholder="nginx:latest" /></label><label class="docker-field"><span>Registry</span><Input v-model="pullDraft.serverAddress" placeholder="registry.example.com" /></label><div class="grid grid-cols-2 gap-3"><label class="docker-field"><span>{{ t("connection.username") }}</span><Input v-model="pullDraft.username" /></label><label class="docker-field"><span>{{ t("connection.password") }}</span><Input v-model="pullDraft.password" type="password" /></label></div><pre v-if="pullProgress" class="max-h-36 overflow-auto rounded bg-muted p-2 text-xs">{{ pullProgress }}</pre></div>
        <DialogFooter><Button variant="outline" @click="pullImageOpen = false">{{ t("common.cancel") }}</Button><Button :disabled="submitting || !pullDraft.image.trim()" @click="pullImage">{{ t("docker.pull") }}</Button></DialogFooter>
      </DialogContent>
    </Dialog>

    <Dialog v-model:open="createVolumeOpen">
      <DialogContent><DialogHeader><DialogTitle>{{ t("docker.createVolume") }}</DialogTitle></DialogHeader><div class="space-y-3 py-2"><label class="docker-field"><span>{{ t("docker.name") }}</span><Input v-model="volumeDraft.name" /></label><label class="docker-field"><span>Driver</span><Input v-model="volumeDraft.driver" /></label><label class="docker-field"><span>Labels</span><textarea v-model="volumeDraft.labels" rows="3" class="docker-textarea" placeholder="key=value" /></label><label class="docker-field"><span>Driver options</span><textarea v-model="volumeDraft.driverOptions" rows="3" class="docker-textarea" placeholder="key=value" /></label></div><DialogFooter><Button variant="outline" @click="createVolumeOpen = false">{{ t("common.cancel") }}</Button><Button :disabled="submitting || !volumeDraft.name.trim()" @click="createVolume">{{ t("docker.create") }}</Button></DialogFooter></DialogContent>
    </Dialog>

    <Dialog v-model:open="createNetworkOpen">
      <DialogContent><DialogHeader><DialogTitle>{{ t("docker.createNetwork") }}</DialogTitle></DialogHeader><div class="grid grid-cols-2 gap-3 py-2"><label class="docker-field"><span>{{ t("docker.name") }}</span><Input v-model="networkDraft.name" /></label><label class="docker-field"><span>Driver</span><Input v-model="networkDraft.driver" /></label><label class="docker-field"><span>Subnet</span><Input v-model="networkDraft.subnet" placeholder="172.28.0.0/16" /></label><label class="docker-field"><span>Gateway</span><Input v-model="networkDraft.gateway" placeholder="172.28.0.1" /></label><label class="flex items-center gap-2 text-sm"><input v-model="networkDraft.internal" type="checkbox" />Internal</label><label class="flex items-center gap-2 text-sm"><input v-model="networkDraft.attachable" type="checkbox" />Attachable</label></div><DialogFooter><Button variant="outline" @click="createNetworkOpen = false">{{ t("common.cancel") }}</Button><Button :disabled="submitting || !networkDraft.name.trim()" @click="createNetwork">{{ t("docker.create") }}</Button></DialogFooter></DialogContent>
    </Dialog>
  </div>
</template>

<style scoped>
.docker-header { background: var(--docker-accent-soft, color-mix(in srgb, var(--muted) 24%, transparent)); }
.docker-main-tab { height: 2.5rem; border-bottom: 2px solid transparent; padding: 0 0.9rem; font-size: 0.875rem; color: var(--muted-foreground); }
.docker-main-tab:hover { color: var(--foreground); }
.docker-main-tab.active { border-color: var(--docker-accent, var(--primary)); color: var(--foreground); font-weight: 600; }
.docker-detail-tab { border-bottom: 2px solid transparent; padding: 0.65rem 1rem; font-size: 0.8rem; color: var(--muted-foreground); }
.docker-detail-tab.active { border-color: var(--docker-accent, var(--primary)); color: var(--foreground); font-weight: 600; }
.docker-table { width: 100%; min-width: max-content; border-collapse: collapse; text-align: left; font-size: 0.875rem; }
.docker-table th { position: sticky; top: 0; z-index: 5; border-bottom: 1px solid var(--border); background: var(--background); padding: 0.65rem 1rem; font-size: 0.75rem; font-weight: 500; color: var(--muted-foreground); }
.docker-table td { border-bottom: 1px solid var(--border); padding: 0.55rem 1rem; vertical-align: middle; }
.docker-table tbody tr:hover { background: color-mix(in srgb, var(--muted) 42%, transparent); }
.docker-card { display: flex; min-width: 0; flex-direction: column; gap: 0.2rem; border: 1px solid var(--border); border-radius: var(--radius-md); background: color-mix(in srgb, var(--muted) 20%, transparent); padding: 0.8rem; }
.docker-card span, .docker-card small { font-size: 0.72rem; color: var(--muted-foreground); }
.docker-card strong { overflow-wrap: anywhere; font-size: 0.875rem; }
.docker-row-value, .docker-code { border: 1px solid var(--border); border-radius: var(--radius-md); background: color-mix(in srgb, var(--muted) 18%, transparent); padding: 0.55rem 0.75rem; font-family: var(--dbx-editor-font-family, ui-monospace); font-size: 0.75rem; }
.docker-code { max-height: 16rem; overflow: auto; white-space: pre-wrap; }
.docker-field { display: flex; flex-direction: column; gap: 0.35rem; font-size: 0.8rem; }
.docker-field > span { color: var(--muted-foreground); }
.docker-textarea, .docker-select { width: 100%; border: 1px solid var(--input); border-radius: var(--radius-md); background: var(--background); padding: 0.5rem 0.65rem; font-size: 0.8rem; outline: none; }
.docker-textarea { resize: vertical; font-family: var(--dbx-editor-font-family, ui-monospace); }
.docker-textarea:focus, .docker-select:focus { border-color: var(--ring); box-shadow: 0 0 0 3px color-mix(in srgb, var(--ring) 22%, transparent); }
</style>
