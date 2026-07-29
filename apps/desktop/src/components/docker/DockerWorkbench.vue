<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { Box, ChevronDown, ChevronRight, CircleStop, Container, Copy, Database, Image, Network, Play, RefreshCw, RotateCw, Search } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import JsonTree from "@/components/common/JsonTree.vue";
import MetricLineChart from "@/components/chart/MetricLineChart.vue";
import { copyToClipboard } from "@/lib/common/clipboard";
import { useToast } from "@/composables/useToast";
import * as api from "@/lib/backend/api";
import type { ConnectionConfig } from "@/types/database";
import type { DockerContainer, DockerContainerAction, DockerContainerStats, DockerImage, DockerNetwork, DockerVolume } from "@/types/docker";

const props = defineProps<{
  connection: ConnectionConfig;
}>();

type ResourceKind = "containers" | "images" | "volumes" | "networks";
type DetailTab = "summary" | "trends" | "inspect";
type TrendPoint = DockerContainerStats;

const { toast } = useToast();
const resource = ref<ResourceKind>("containers");
const loading = ref(false);
const error = ref("");
const query = ref("");
const containers = ref<DockerContainer[]>([]);
const images = ref<DockerImage[]>([]);
const volumes = ref<DockerVolume[]>([]);
const networks = ref<DockerNetwork[]>([]);
const listStats = ref<Record<string, DockerContainerStats>>({});
const selectedContainerId = ref("");
const inspect = ref<unknown>();
const inspectLoading = ref(false);
const inspectSearch = ref("");
const jsonTreeRef = ref<{ expandAll: () => void; collapseAll: () => void }>();
const detailTab = ref<DetailTab>("summary");
const trend = ref<TrendPoint[]>([]);
const actionInFlight = ref<Record<string, DockerContainerAction | undefined>>({});
const expandedProjects = ref(new Set<string>());
let listStatsTimer: number | undefined;
let detailStatsTimer: number | undefined;

const selectedContainer = computed(() => containers.value.find((container) => container.id === selectedContainerId.value));
const normalizedQuery = computed(() => query.value.trim().toLowerCase());
const filteredContainers = computed(() => {
  if (!normalizedQuery.value) return containers.value;
  return containers.value.filter((container) => {
    const haystack = [container.id, container.image, container.state, container.status, ...container.names, ...Object.values(container.labels)].join(" ").toLowerCase();
    return haystack.includes(normalizedQuery.value);
  });
});
const filteredImages = computed(() => {
  if (!normalizedQuery.value) return images.value;
  return images.value.filter((item) => [item.id, ...item.repoTags, ...item.repoDigests].join(" ").toLowerCase().includes(normalizedQuery.value));
});
const filteredVolumes = computed(() => {
  if (!normalizedQuery.value) return volumes.value;
  return volumes.value.filter((item) => [item.name, item.driver, item.scope, item.mountpoint].join(" ").toLowerCase().includes(normalizedQuery.value));
});
const filteredNetworks = computed(() => {
  if (!normalizedQuery.value) return networks.value;
  return networks.value.filter((item) => [item.id, item.name, item.driver, item.scope].join(" ").toLowerCase().includes(normalizedQuery.value));
});
const groupedContainers = computed(() => {
  const groups = new Map<string, DockerContainer[]>();
  for (const container of containers.value) {
    const project = container.labels["com.docker.compose.project"] || "Ungrouped";
    const values = groups.get(project) ?? [];
    values.push(container);
    groups.set(project, values);
  }
  return [...groups.entries()].sort(([left], [right]) => {
    if (left === "Ungrouped") return 1;
    if (right === "Ungrouped") return -1;
    return left.localeCompare(right);
  });
});
const inspectRecord = computed<Record<string, any>>(() => {
  return inspect.value && typeof inspect.value === "object" && !Array.isArray(inspect.value) ? (inspect.value as Record<string, any>) : {};
});
const trendLabels = computed(() => trend.value.map((point) => new Date(point.readAt).toLocaleTimeString()));
const cpuSeries = computed(() => [{ name: "CPU", data: trend.value.map((point) => point.cpuPercent), color: "#3b82f6" }]);
const memorySeries = computed(() => [
  { name: "Usage", data: trend.value.map((point) => point.memoryUsage), color: "#8b5cf6" },
  { name: "Limit", data: trend.value.map((point) => point.memoryLimit), color: "#94a3b8" },
]);
const memoryPercentSeries = computed(() => [{ name: "Memory", data: trend.value.map((point) => point.memoryPercent), color: "#8b5cf6" }]);
const networkSeries = computed(() => [
  { name: "RX", data: trend.value.map((point) => point.networkRx), color: "#22c55e" },
  { name: "TX", data: trend.value.map((point) => point.networkTx), color: "#f59e0b" },
]);
const blockSeries = computed(() => [
  { name: "Read", data: trend.value.map((point) => point.blockRead), color: "#06b6d4" },
  { name: "Write", data: trend.value.map((point) => point.blockWrite), color: "#ef4444" },
]);

function containerName(container: DockerContainer): string {
  return container.labels["com.docker.compose.service"] || container.names[0]?.replace(/^\//, "") || container.id.slice(0, 12);
}

function shortId(id: string): string {
  return id.replace(/^sha256:/, "").slice(0, 12);
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatDate(timestamp: number): string {
  return timestamp ? new Date(timestamp * 1000).toLocaleString() : "—";
}

function formatPorts(container: DockerContainer): string {
  return container.ports.map((port) => (port.publicPort ? `${port.ip || "0.0.0.0"}:${port.publicPort}→${port.privatePort}/${port.portType}` : `${port.privatePort}/${port.portType}`)).join(", ") || "—";
}

function containerIp(container: DockerContainer): string {
  return Object.values(container.networkIps).filter(Boolean).join(", ") || "—";
}

function isRunning(container: DockerContainer): boolean {
  return container.state.toLowerCase() === "running";
}

function canAction(container: DockerContainer, action: DockerContainerAction): boolean {
  if (props.connection.read_only || actionInFlight.value[container.id]) return false;
  return action === "start" ? !isRunning(container) : isRunning(container);
}

function setProjectExpanded(project: string) {
  const next = new Set(expandedProjects.value);
  if (next.has(project)) {
    next.delete(project);
  } else {
    next.add(project);
  }
  expandedProjects.value = next;
}

async function loadContainers() {
  containers.value = await api.dockerListContainers(props.connection.id, true);
  if (selectedContainerId.value && !containers.value.some((container) => container.id === selectedContainerId.value)) {
    closeDetail();
  }
}

async function loadResource(kind = resource.value) {
  loading.value = true;
  error.value = "";
  try {
    if (kind === "containers") await loadContainers();
    if (kind === "images") images.value = await api.dockerListImages(props.connection.id);
    if (kind === "volumes") volumes.value = await api.dockerListVolumes(props.connection.id);
    if (kind === "networks") networks.value = await api.dockerListNetworks(props.connection.id);
    if (kind === "containers") await sampleVisibleContainers();
  } catch (cause: any) {
    error.value = cause?.message || String(cause);
  } finally {
    loading.value = false;
  }
}

async function selectResource(kind: ResourceKind) {
  resource.value = kind;
  query.value = "";
  if ((kind === "containers" && !containers.value.length) || (kind === "images" && !images.value.length) || (kind === "volumes" && !volumes.value.length) || (kind === "networks" && !networks.value.length)) {
    await loadResource(kind);
  }
}

async function selectContainer(container: DockerContainer) {
  selectedContainerId.value = container.id;
  inspect.value = undefined;
  trend.value = [];
  detailTab.value = "summary";
  await loadInspect();
  restartDetailSampling();
}

async function loadInspect() {
  if (!selectedContainerId.value) return;
  inspectLoading.value = true;
  try {
    inspect.value = await api.dockerInspectContainer(props.connection.id, selectedContainerId.value);
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    inspectLoading.value = false;
  }
}

function closeDetail() {
  selectedContainerId.value = "";
  inspect.value = undefined;
  trend.value = [];
  stopDetailSampling();
}

async function runAction(container: DockerContainer, action: DockerContainerAction) {
  if (!canAction(container, action)) return;
  const dangerous = action !== "start" || props.connection.is_production;
  if (dangerous && !window.confirm(`Confirm ${action} for container "${containerName(container)}"?`)) return;
  actionInFlight.value = { ...actionInFlight.value, [container.id]: action };
  try {
    await api.dockerContainerAction(props.connection.id, container.id, action);
    toast(`Container ${action} succeeded`);
    await loadContainers();
    if (selectedContainerId.value === container.id) await loadInspect();
  } catch (cause: any) {
    toast(cause?.message || String(cause), 5000);
  } finally {
    const next = { ...actionInFlight.value };
    delete next[container.id];
    actionInFlight.value = next;
  }
}

async function sampleVisibleContainers() {
  if (document.hidden || resource.value !== "containers") return;
  const ids = filteredContainers.value
    .filter(isRunning)
    .slice(0, 8)
    .map((container) => container.id);
  if (!ids.length) return;
  try {
    const stats = await api.dockerContainerStats(props.connection.id, ids);
    listStats.value = Object.fromEntries(stats.map((point) => [point.containerId, point]));
  } catch {
    // A daemon restart or a container stopping between list and stats is expected.
  }
}

async function sampleSelectedContainer() {
  const container = selectedContainer.value;
  if (document.hidden || !container || !isRunning(container)) return;
  try {
    const [point] = await api.dockerContainerStats(props.connection.id, [container.id]);
    if (!point) return;
    const cutoff = Date.now() - 15 * 60 * 1000;
    trend.value = [...trend.value, point].filter((item) => new Date(item.readAt).getTime() >= cutoff);
  } catch {
    // Keep the last trend visible while a transient connection failure recovers.
  }
}

function stopDetailSampling() {
  if (detailStatsTimer !== undefined) window.clearInterval(detailStatsTimer);
  detailStatsTimer = undefined;
}

function restartDetailSampling() {
  stopDetailSampling();
  void sampleSelectedContainer();
  detailStatsTimer = window.setInterval(() => void sampleSelectedContainer(), 2000);
}

function restartListSampling() {
  if (listStatsTimer !== undefined) window.clearInterval(listStatsTimer);
  void sampleVisibleContainers();
  listStatsTimer = window.setInterval(() => void sampleVisibleContainers(), 5000);
}

function onVisibilityChange() {
  if (!document.hidden) {
    void sampleVisibleContainers();
    void sampleSelectedContainer();
  }
}

async function copyInspect() {
  if (inspect.value === undefined) return;
  await copyToClipboard(JSON.stringify(inspect.value, null, 2));
  toast("Inspect JSON copied");
}

function highlightInspectJson(value: string): string {
  const escaped = value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const needle = inspectSearch.value.trim();
  if (!needle) return escaped;
  const escapedNeedle = needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return escaped.replace(new RegExp(escapedNeedle, "gi"), (match) => `<mark class="rounded bg-amber-300/60 text-inherit">${match}</mark>`);
}

watch(
  () => props.connection.id,
  async () => {
    containers.value = [];
    images.value = [];
    volumes.value = [];
    networks.value = [];
    closeDetail();
    await loadResource("containers");
  },
);
watch(resource, restartListSampling);

onMounted(async () => {
  document.addEventListener("visibilitychange", onVisibilityChange);
  await loadResource("containers");
  restartListSampling();
});

onUnmounted(() => {
  document.removeEventListener("visibilitychange", onVisibilityChange);
  if (listStatsTimer !== undefined) window.clearInterval(listStatsTimer);
  stopDetailSampling();
});
</script>

<template>
  <div class="flex h-full min-h-0 bg-background">
    <aside class="flex w-64 shrink-0 flex-col border-r bg-muted/10">
      <div class="border-b px-3 py-3">
        <div class="flex items-center gap-2 font-semibold">
          <Container class="h-4 w-4 text-sky-500" />
          <span class="truncate">{{ connection.name }}</span>
        </div>
        <div class="mt-1 text-xs text-muted-foreground">Docker Workbench</div>
      </div>
      <nav class="min-h-0 flex-1 overflow-auto p-2 text-sm">
        <button class="resource-node" :class="{ active: resource === 'containers' }" @click="selectResource('containers')">
          <Container class="h-4 w-4 text-sky-500" /><span>Containers</span><span class="ml-auto count">{{ containers.length }}</span>
        </button>
        <div v-if="resource === 'containers'" class="ml-4 border-l pl-1">
          <div v-for="[project, values] in groupedContainers" :key="project">
            <button class="resource-node w-full" @click="setProjectExpanded(project)">
              <ChevronDown v-if="expandedProjects.has(project)" class="h-3.5 w-3.5" />
              <ChevronRight v-else class="h-3.5 w-3.5" />
              <Box class="h-3.5 w-3.5 text-sky-500" />
              <span class="truncate">{{ project }}</span
              ><span class="ml-auto count">{{ values.length }}</span>
            </button>
            <button v-for="container in expandedProjects.has(project) ? values : []" :key="container.id" class="resource-node ml-5 w-[calc(100%-1.25rem)]" :class="{ active: selectedContainerId === container.id }" @click="selectContainer(container)">
              <span class="h-2 w-2 rounded-full" :class="isRunning(container) ? 'bg-emerald-500' : 'bg-zinc-400'" />
              <span class="truncate">{{ containerName(container) }}</span>
            </button>
          </div>
        </div>
        <button class="resource-node" :class="{ active: resource === 'images' }" @click="selectResource('images')">
          <Image class="h-4 w-4 text-indigo-500" /><span>Images</span><span class="ml-auto count">{{ images.length }}</span>
        </button>
        <button class="resource-node" :class="{ active: resource === 'volumes' }" @click="selectResource('volumes')">
          <Database class="h-4 w-4 text-amber-500" /><span>Volumes</span><span class="ml-auto count">{{ volumes.length }}</span>
        </button>
        <button class="resource-node" :class="{ active: resource === 'networks' }" @click="selectResource('networks')">
          <Network class="h-4 w-4 text-emerald-500" /><span>Networks</span><span class="ml-auto count">{{ networks.length }}</span>
        </button>
      </nav>
    </aside>

    <main class="flex min-w-0 flex-1 flex-col">
      <header class="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <Button size="sm" variant="ghost" :disabled="loading" @click="loadResource()"> <RefreshCw :class="{ 'animate-spin': loading }" /> Refresh </Button>
        <div class="relative ml-auto w-72 max-w-[40%]">
          <Search class="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input v-model="query" class="h-8 pl-7" placeholder="Filter resources" />
        </div>
      </header>

      <div v-if="error" class="m-3 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">{{ error }}</div>
      <div v-else class="min-h-0 flex-1 overflow-auto">
        <table v-if="resource === 'containers'" class="resource-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>ID</th>
              <th>Image</th>
              <th>CPU</th>
              <th>Memory</th>
              <th>Status</th>
              <th>IP</th>
              <th>Ports</th>
              <th>Created</th>
              <th class="sticky right-0 bg-background">Actions</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="container in filteredContainers" :key="container.id" :class="{ selected: selectedContainerId === container.id }" @dblclick="selectContainer(container)">
              <td>
                <button class="font-medium hover:underline" @click="selectContainer(container)">{{ containerName(container) }}</button>
              </td>
              <td class="font-mono text-xs">{{ shortId(container.id) }}</td>
              <td class="max-w-56 truncate">{{ container.image }}</td>
              <td>{{ listStats[container.id] ? `${listStats[container.id].cpuPercent.toFixed(1)}%` : "—" }}</td>
              <td>{{ listStats[container.id] ? `${formatBytes(listStats[container.id].memoryUsage)} / ${formatBytes(listStats[container.id].memoryLimit)}` : "—" }}</td>
              <td>
                <span class="status-pill" :class="isRunning(container) ? 'running' : 'stopped'">{{ container.state }}</span>
                <div class="mt-1 max-w-44 truncate text-xs text-muted-foreground">{{ container.status }}</div>
              </td>
              <td>{{ containerIp(container) }}</td>
              <td class="max-w-72 truncate">{{ formatPorts(container) }}</td>
              <td>{{ formatDate(container.created) }}</td>
              <td class="sticky right-0 bg-background">
                <div class="flex gap-1">
                  <Button size="icon-xs" variant="ghost" title="Start" :disabled="!canAction(container, 'start')" @click="runAction(container, 'start')"><Play /></Button>
                  <Button size="icon-xs" variant="ghost" title="Stop" :disabled="!canAction(container, 'stop')" @click="runAction(container, 'stop')"><CircleStop /></Button>
                  <Button size="icon-xs" variant="ghost" title="Restart" :disabled="!canAction(container, 'restart')" @click="runAction(container, 'restart')"><RotateCw /></Button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>

        <table v-else-if="resource === 'images'" class="resource-table">
          <thead>
            <tr>
              <th>Repository / Tag</th>
              <th>ID</th>
              <th>Size</th>
              <th>Created</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in filteredImages" :key="item.id">
              <td>{{ item.repoTags.join(", ") || item.repoDigests.join(", ") || "&lt;none&gt;" }}</td>
              <td class="font-mono text-xs">{{ shortId(item.id) }}</td>
              <td>{{ formatBytes(item.size) }}</td>
              <td>{{ formatDate(item.created) }}</td>
            </tr>
          </tbody>
        </table>
        <table v-else-if="resource === 'volumes'" class="resource-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Driver</th>
              <th>Scope</th>
              <th>Mountpoint</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in filteredVolumes" :key="item.name">
              <td class="font-medium">{{ item.name }}</td>
              <td>{{ item.driver }}</td>
              <td>{{ item.scope }}</td>
              <td class="font-mono text-xs">{{ item.mountpoint }}</td>
            </tr>
          </tbody>
        </table>
        <table v-else class="resource-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>ID</th>
              <th>Driver</th>
              <th>Scope</th>
              <th>Internal</th>
              <th>Attachable</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in filteredNetworks" :key="item.id">
              <td class="font-medium">{{ item.name }}</td>
              <td class="font-mono text-xs">{{ shortId(item.id) }}</td>
              <td>{{ item.driver }}</td>
              <td>{{ item.scope }}</td>
              <td>{{ item.internal ? "Yes" : "No" }}</td>
              <td>{{ item.attachable ? "Yes" : "No" }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </main>

    <aside v-if="selectedContainer" class="flex w-[42%] min-w-[420px] max-w-[720px] shrink-0 flex-col border-l bg-background">
      <header class="flex h-12 items-center gap-2 border-b px-3">
        <div class="min-w-0 flex-1">
          <div class="truncate font-semibold">{{ containerName(selectedContainer) }}</div>
          <div class="font-mono text-[11px] text-muted-foreground">{{ shortId(selectedContainer.id) }}</div>
        </div>
        <button class="text-lg text-muted-foreground hover:text-foreground" @click="closeDetail">×</button>
      </header>
      <div class="flex border-b px-3">
        <button v-for="tab in ['summary', 'trends', 'inspect'] as DetailTab[]" :key="tab" class="detail-tab" :class="{ active: detailTab === tab }" @click="detailTab = tab">{{ tab[0].toUpperCase() + tab.slice(1) }}</button>
      </div>
      <div class="min-h-0 flex-1 overflow-auto p-4">
        <div v-if="detailTab === 'summary'" class="space-y-4">
          <section class="summary-grid">
            <div>
              <span>Status</span><strong>{{ inspectRecord.State?.Status || selectedContainer.state }}</strong>
            </div>
            <div>
              <span>Health</span><strong>{{ inspectRecord.State?.Health?.Status || "—" }}</strong>
            </div>
            <div>
              <span>Image</span><strong>{{ inspectRecord.Config?.Image || selectedContainer.image }}</strong>
            </div>
            <div>
              <span>Restart count</span><strong>{{ inspectRecord.RestartCount ?? "—" }}</strong>
            </div>
            <div class="col-span-2">
              <span>Command</span><strong class="font-mono text-xs">{{ [inspectRecord.Path, ...(inspectRecord.Args || [])].filter(Boolean).join(" ") || selectedContainer.command }}</strong>
            </div>
          </section>
          <section>
            <h3>Ports</h3>
            <div class="detail-value">{{ formatPorts(selectedContainer) }}</div>
          </section>
          <section>
            <h3>Mounts</h3>
            <div v-for="mount in inspectRecord.Mounts || []" :key="`${mount.Source}-${mount.Destination}`" class="detail-value font-mono text-xs">{{ mount.Source }} → {{ mount.Destination }} ({{ mount.Mode || mount.Type }})</div>
            <div v-if="!inspectRecord.Mounts?.length" class="detail-value">—</div>
          </section>
          <section>
            <h3>Networks</h3>
            <div v-for="(network, name) in inspectRecord.NetworkSettings?.Networks || {}" :key="String(name)" class="detail-value">{{ name }} · {{ network.IPAddress || "no IP" }}</div>
            <div v-if="!Object.keys(inspectRecord.NetworkSettings?.Networks || {}).length" class="detail-value">—</div>
          </section>
        </div>
        <div v-else-if="detailTab === 'trends'" class="grid grid-cols-1 gap-3 xl:grid-cols-2">
          <MetricLineChart title="CPU %" :labels="trendLabels" :series="cpuSeries" :value-formatter="(value) => `${value.toFixed(1)}%`" />
          <MetricLineChart title="Memory %" :labels="trendLabels" :series="memoryPercentSeries" :value-formatter="(value) => `${value.toFixed(1)}%`" />
          <MetricLineChart title="Memory usage" :labels="trendLabels" :series="memorySeries" :value-formatter="formatBytes" />
          <MetricLineChart title="Network I/O" :labels="trendLabels" :series="networkSeries" :value-formatter="formatBytes" />
          <MetricLineChart title="Block I/O" :labels="trendLabels" :series="blockSeries" :value-formatter="formatBytes" />
        </div>
        <div v-else class="flex h-full min-h-0 flex-col">
          <div class="mb-2 flex items-center gap-1">
            <div class="relative min-w-0 flex-1">
              <Search class="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input v-model="inspectSearch" class="h-7 pl-7 text-xs" placeholder="Search Inspect JSON" />
            </div>
            <Button size="xs" variant="ghost" @click="jsonTreeRef?.expandAll()">Expand</Button>
            <Button size="xs" variant="ghost" @click="jsonTreeRef?.collapseAll()">Collapse</Button>
            <Button size="sm" variant="ghost" :disabled="inspectLoading" @click="loadInspect"><RefreshCw :class="{ 'animate-spin': inspectLoading }" /> Refresh</Button>
            <Button size="sm" variant="ghost" :disabled="inspect === undefined" @click="copyInspect"><Copy /> Copy</Button>
          </div>
          <JsonTree v-if="inspect !== undefined" ref="jsonTreeRef" :value="inspect" :highlight-json="highlightInspectJson" :initial-expanded-depth="2" :virtualized="true" class="min-h-0 flex-1 rounded border p-2 font-mono text-xs" />
        </div>
      </div>
    </aside>
  </div>
</template>

<style scoped>
.resource-node {
  @apply flex h-8 items-center gap-2 rounded px-2 text-left text-muted-foreground hover:bg-muted hover:text-foreground;
}
.resource-node.active {
  @apply bg-muted font-medium text-foreground;
}
.count {
  @apply rounded bg-muted-foreground/10 px-1.5 text-[11px] tabular-nums;
}
.resource-table {
  @apply w-full min-w-max border-collapse text-left text-sm;
}
.resource-table th {
  @apply sticky top-0 z-10 border-b bg-background px-3 py-2 text-xs font-medium text-muted-foreground;
}
.resource-table td {
  @apply border-b px-3 py-2 align-middle;
}
.resource-table tbody tr:hover,
.resource-table tbody tr.selected {
  @apply bg-muted/40;
}
.status-pill {
  @apply rounded px-2 py-0.5 text-xs font-medium;
}
.status-pill.running {
  @apply bg-emerald-500/10 text-emerald-600;
}
.status-pill.stopped {
  @apply bg-zinc-500/10 text-zinc-500;
}
.detail-tab {
  @apply border-b-2 border-transparent px-3 py-2 text-xs font-medium capitalize text-muted-foreground;
}
.detail-tab.active {
  @apply border-primary text-foreground;
}
.summary-grid {
  @apply grid grid-cols-2 gap-3;
}
.summary-grid > div {
  @apply flex min-w-0 flex-col rounded border bg-muted/20 p-3;
}
.summary-grid span,
section h3 {
  @apply mb-1 text-xs font-medium text-muted-foreground;
}
.summary-grid strong {
  @apply break-words text-sm;
}
.detail-value {
  @apply rounded border bg-muted/20 px-3 py-2 text-sm;
}
</style>
