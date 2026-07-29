<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowDown, ArrowLeftRight, ArrowUp, ArrowUpDown, ClipboardPaste, Columns3, Copy, Download, Eraser, File, FileDown, FileText, FileUp, Folder, FolderPlus, Home, ListChecks, Loader2, Pencil, RefreshCw, TextSelect, Trash2, X } from "@lucide/vue";
import { Pane, Splitpanes } from "splitpanes";
import type { Detection as ZmodemDetection, Session as ZmodemSession, Sentry as ZmodemSentry } from "zmodem.js";
import "@xterm/xterm/css/xterm.css";
import "splitpanes/dist/splitpanes.css";
import * as api from "@/lib/backend/api";
import { safeLocalStorageGet, safeLocalStorageSet } from "@/lib/backend/safeStorage";
import { hexToRgba } from "@/lib/common/color";
import { copyToClipboard, readTextFromClipboard } from "@/lib/common/clipboard";
import { formatObjectBrowserBytes, formatObjectBrowserTimestamp } from "@/lib/table/objectBrowserRows";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToast } from "@/composables/useToast";
import { useTheme } from "@/composables/useTheme";
import type { ConnectionConfig, QueryTab, SftpEntry, SftpTransferTask } from "@/types/database";
import { Osc7DirectoryParser } from "@/lib/ssh/terminalDirectoryTracking";
import { shouldReattachTerminal, terminalReconnectDelay } from "@/lib/ssh/terminalReconnect";
import { createZmodemSentry, sendZmodemFiles, type ZmodemUploadProgress } from "@/lib/ssh/terminalZmodem";
import { getSshWorkbenchSplitLayout, type SshWorkbenchPaneOrder } from "@/lib/ssh/workbenchLayout";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import DangerConfirmDialog from "@/components/editor/DangerConfirmDialog.vue";
import LightTooltip from "@/components/ui/LightTooltip.vue";
import CustomContextMenu, { type ContextMenuItem } from "@/components/ui/CustomContextMenu.vue";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import SshTextPreview from "@/components/ssh/SshTextPreview.vue";
import DatabaseIcon from "@/components/icons/DatabaseIcon.vue";

const props = defineProps<{
  tab: QueryTab;
  connection: ConnectionConfig;
}>();

const settingsStore = useSettingsStore();
const { isDark, themePalette } = useTheme();
const { toast } = useToast();
const { t } = useI18n();
const terminalHost = ref<HTMLElement | null>(null);
const zmodemFileInput = ref<HTMLInputElement | null>(null);
const terminalState = ref<"connecting" | "connected" | "disconnected" | "error">("connecting");
const terminalError = ref("");
const sftpError = ref("");
const sftpLoading = ref(false);
const entries = ref<SftpEntry[]>([]);
const currentPath = ref(props.tab.sshSftpPath || "");
const splitRatio = ref(readSplitRatio());
const paneOrder = ref<SshWorkbenchPaneOrder>(readPaneOrder());
const operationDialog = ref<"mkdir" | null>(null);
const operationDraft = ref("");
const deleteTarget = ref<SftpEntry | null>(null);
const deleteSubmitting = ref(false);
const previewOpen = ref(false);
const previewTitle = ref("");
const previewText = ref("");
const previewLoading = ref(false);
const sftpDragActive = ref(false);
const transferTasks = ref<Record<string, SftpTransferTask>>({});
const transferPopoverOpen = ref(false);
const transferSpeeds = ref<Record<string, number>>({});
const selectedSftpPath = ref("");
const visibleSftpColumns = ref<SftpColumn[]>(readVisibleSftpColumns());
const sftpSort = ref<{ column: SftpSortColumn; direction: "asc" | "desc" }>({ column: "name", direction: "asc" });
const pendingTerminalInput = ref("");
const renamingPath = ref("");
const renameDraft = ref("");
const renameSubmitting = ref(false);
const renameInput = ref<InstanceType<typeof Input> | null>(null);
const pendingFollowPath = ref("");
const failedFollowPath = ref("");
const zmodemState = ref<"idle" | "waiting" | "uploading">("idle");
const zmodemFileName = ref("");
const zmodemTransferred = ref(0);
const zmodemTotalSize = ref(0);
const zmodemSpeed = ref(0);

let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let terminalSocket: WebSocket | null = null;
let zmodemSentry: ZmodemSentry | null = null;
let zmodemSession: ZmodemSession | null = null;
let pendingZmodemFiles: File[] = [];
let zmodemDetectionTimer = 0;
let zmodemSampledAt = 0;
let zmodemSampledBytes = 0;
let resizeObserver: ResizeObserver | null = null;
let resizeTimer = 0;
let terminalReconnectTimer = 0;
let terminalReconnectAttempt = 0;
let disposed = false;
let mounted = false;
let unlistenTransferProgress: (() => void) | null = null;
let lastCols = 0;
let lastRows = 0;
let directoryRequestSequence = 0;
const transferSamples = new Map<string, { transferred: number; sampledAt: number; speed: number }>();
const directoryParser = new Osc7DirectoryParser();

type SftpColumn = "size" | "modified" | "permissions";
type SftpSortColumn = "name" | "size" | "modified";
const SFTP_COLUMNS_STORAGE_KEY = "dbx:ssh-workbench:sftp-columns";
const PANE_ORDER_STORAGE_KEY = "dbx:ssh-workbench:pane-order";
const MAX_INLINE_PREVIEW_BYTES = 1024 * 1024;
const PREVIEWABLE_EXTENSIONS = new Set(["bash", "bat", "c", "cfg", "cmd", "conf", "cpp", "css", "csv", "go", "h", "hpp", "htm", "html", "ini", "java", "js", "json", "log", "md", "properties", "ps1", "py", "rs", "scss", "sh", "sql", "toml", "ts", "tsx", "txt", "vue", "xml", "yaml", "yml", "zsh"]);
const ZMODEM_DETECTION_TIMEOUT_MS = 5000;
const canWrite = computed(() => !props.connection.read_only);
const zmodemBusy = computed(() => zmodemState.value !== "idle");
const zmodemPercent = computed(() => (zmodemTotalSize.value > 0 ? Math.min(100, Math.round((zmodemTransferred.value / zmodemTotalSize.value) * 100)) : 0));
const splitLayout = computed(() => getSshWorkbenchSplitLayout(paneOrder.value));
const editorSettings = computed(() => settingsStore.editorSettings);
const connectionIdentity = computed(() => {
  const host = props.connection.host?.trim() || props.connection.name;
  const username = props.connection.username?.trim();
  return username ? `${username}@${host}` : host;
});
const toolbarStyle = computed(() => {
  const color = props.connection.color;
  if (!color) return undefined;
  return {
    backgroundColor: hexToRgba(color, 0.1),
    boxShadow: `inset 0 1px 0 ${hexToRgba(color, 0.18)}`,
  };
});
const sftpGridStyle = computed(() => ({
  gridTemplateColumns: ["minmax(120px, 1fr)", visibleSftpColumns.value.includes("size") ? "72px" : "", visibleSftpColumns.value.includes("modified") ? "128px" : "", visibleSftpColumns.value.includes("permissions") ? "84px" : ""].filter(Boolean).join(" "),
  minWidth: `${180 + (visibleSftpColumns.value.includes("size") ? 78 : 0) + (visibleSftpColumns.value.includes("modified") ? 134 : 0) + (visibleSftpColumns.value.includes("permissions") ? 90 : 0)}px`,
}));
const sortedEntries = computed(() => {
  const { column, direction } = sftpSort.value;
  const multiplier = direction === "asc" ? 1 : -1;
  return [...entries.value].sort((left, right) => {
    if (left.kind === "directory" && right.kind !== "directory") return -1;
    if (left.kind !== "directory" && right.kind === "directory") return 1;
    let result = 0;
    if (column === "size") result = (left.size ?? -1) - (right.size ?? -1);
    else if (column === "modified") result = (left.modifiedAt ?? 0) - (right.modifiedAt ?? 0);
    else result = left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" });
    return result * multiplier;
  });
});
const terminalBackground = computed(() => resolveTerminalTheme().background);

function readVisibleSftpColumns(): SftpColumn[] {
  const stored = safeLocalStorageGet(SFTP_COLUMNS_STORAGE_KEY);
  if (stored === null) return ["size", "modified"];
  try {
    const value = JSON.parse(stored);
    if (Array.isArray(value)) {
      return value.filter((column): column is SftpColumn => ["size", "modified", "permissions"].includes(column));
    }
  } catch {
    // Invalid old state falls back to DBX defaults.
  }
  return ["size", "modified"];
}

function toggleSftpColumn(column: SftpColumn, checked: boolean) {
  visibleSftpColumns.value = checked ? Array.from(new Set([...visibleSftpColumns.value, column])) : visibleSftpColumns.value.filter((value) => value !== column);
  safeLocalStorageSet(SFTP_COLUMNS_STORAGE_KEY, JSON.stringify(visibleSftpColumns.value));
}

function toggleSftpColumnFromMenu(event: Event, column: SftpColumn) {
  event.preventDefault();
  toggleSftpColumn(column, !visibleSftpColumns.value.includes(column));
}

function toggleSftpSort(column: SftpSortColumn) {
  sftpSort.value = sftpSort.value.column === column ? { column, direction: sftpSort.value.direction === "asc" ? "desc" : "asc" } : { column, direction: "asc" };
}

function sortIcon(column: SftpSortColumn) {
  if (sftpSort.value.column !== column) return ArrowUpDown;
  return sftpSort.value.direction === "asc" ? ArrowUp : ArrowDown;
}

function isInlinePreviewSupported(entry: SftpEntry): boolean {
  if ((entry.size ?? 0) > MAX_INLINE_PREVIEW_BYTES) return false;
  const extension = entry.name.includes(".") ? entry.name.split(".").pop()?.toLowerCase() : "";
  return !!extension && PREVIEWABLE_EXTENSIONS.has(extension);
}

function readSplitRatio(): number {
  const stored = Number(safeLocalStorageGet("dbx:ssh-workbench:split-ratio"));
  return Number.isFinite(stored) && stored >= 35 && stored <= 80 ? stored : 68;
}

function onSplitResize(event: { panes?: Array<{ size: number }> } | Array<{ size: number }>) {
  const panes = Array.isArray(event) ? event : event.panes;
  const size = panes?.[0]?.size;
  if (typeof size === "number" && Number.isFinite(size)) {
    splitRatio.value = size;
    safeLocalStorageSet("dbx:ssh-workbench:split-ratio", String(size));
  }
  scheduleFit();
}

function readPaneOrder(): SshWorkbenchPaneOrder {
  return safeLocalStorageGet(PANE_ORDER_STORAGE_KEY) === "sftp-left" ? "sftp-left" : "terminal-left";
}

function togglePaneOrder() {
  paneOrder.value = paneOrder.value === "terminal-left" ? "sftp-left" : "terminal-left";
  safeLocalStorageSet(PANE_ORDER_STORAGE_KEY, paneOrder.value);
  void nextTick(scheduleFit);
}

const paneOrderToggleLabel = computed(() => t(paneOrder.value === "terminal-left" ? "sshWorkbench.moveSftpLeft" : "sshWorkbench.moveTerminalLeft"));

function parentPath(path: string): string {
  const normalized = path.replace(/\/+$/, "");
  if (!normalized || normalized === "/") return "/";
  const index = normalized.lastIndexOf("/");
  return index <= 0 ? "/" : normalized.slice(0, index);
}

function joinRemotePath(parent: string, name: string): string {
  const base = parent === "/" ? "" : parent.replace(/\/+$/, "");
  return `${base}/${name.replace(/^\/+/, "")}`;
}

function normalizeRemotePath(path: string): string {
  let decoded = path.trim();
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // Keep the original path when the shell emits a literal percent sign.
  }
  if (!decoded) return "/";
  if (!decoded.startsWith("/")) decoded = `/${decoded}`;
  decoded = decoded.replace(/\/{2,}/g, "/");
  return decoded === "/" ? decoded : decoded.replace(/\/+$/, "");
}

function localFileName(source: string | File): string {
  if (typeof source !== "string") return source.name;
  return source.split(/[\\/]/).pop() || "upload";
}

function formattedModifiedAt(value?: number | null): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return "";
  return formatObjectBrowserTimestamp(new Date(value * 1000).toISOString());
}

function cssToken(name: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

function resolveTerminalTheme() {
  const background = cssToken("--background", isDark.value ? "#0b1117" : "#ffffff");
  const foreground = cssToken("--foreground", isDark.value ? "#d9e1e8" : "#111827");
  const primary = cssToken("--primary", isDark.value ? "#60a5fa" : "#2563eb");
  const muted = cssToken("--muted-foreground", isDark.value ? "#94a3b8" : "#64748b");
  return {
    background,
    foreground,
    cursor: primary,
    cursorAccent: background,
    selectionBackground: cssToken("--accent", isDark.value ? "#334155" : "#dbeafe"),
    black: isDark.value ? "#1f2937" : "#111827",
    red: "#ef4444",
    green: "#22c55e",
    yellow: "#eab308",
    blue: "#3b82f6",
    magenta: "#a855f7",
    cyan: "#06b6d4",
    white: foreground,
    brightBlack: muted,
    brightRed: "#f87171",
    brightGreen: "#4ade80",
    brightYellow: "#facc15",
    brightBlue: "#60a5fa",
    brightMagenta: "#c084fc",
    brightCyan: "#22d3ee",
    brightWhite: foreground,
  };
}

function terminalOptions() {
  return {
    convertEol: false,
    cursorBlink: true,
    cursorStyle: "block" as const,
    fontFamily: editorSettings.value.fontFamily,
    fontSize: editorSettings.value.fontSize,
    lineHeight: 1.15,
    scrollback: 10_000,
    allowProposedApi: false,
    theme: resolveTerminalTheme(),
  };
}

function createTerminal() {
  if (!terminalHost.value || terminal) return;
  terminal = new Terminal(terminalOptions());
  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(terminalHost.value);
  terminal.onData((data) => {
    if (zmodemBusy.value) return;
    trackPendingTerminalInput(data);
    if (terminalSocket?.readyState === WebSocket.OPEN) {
      terminalSocket.send(new TextEncoder().encode(data));
    }
  });
  resizeObserver = new ResizeObserver(scheduleFit);
  resizeObserver.observe(terminalHost.value);
  scheduleFit();
}

function trackPendingTerminalInput(data: string) {
  if (data.includes("\u001b")) return;
  for (const character of data) {
    if (character === "\r" || character === "\n" || character === "\u0003") {
      pendingTerminalInput.value = "";
    } else if (character === "\u007f") {
      pendingTerminalInput.value = pendingTerminalInput.value.slice(0, -1);
    } else if (character >= " " && character !== "\u001b") {
      pendingTerminalInput.value += character;
    }
  }
}

function scheduleFit() {
  window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => {
    if (!terminal || !fitAddon || !terminalHost.value || terminalHost.value.clientWidth === 0 || terminalHost.value.clientHeight === 0) return;
    try {
      fitAddon.fit();
      if (terminal.cols !== lastCols || terminal.rows !== lastRows) {
        lastCols = terminal.cols;
        lastRows = terminal.rows;
        if (terminalSocket?.readyState === WebSocket.OPEN) {
          terminalSocket.send(JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }));
        }
      }
    } catch {
      // The host can briefly be detached while DBX switches tabs.
    }
  }, 20);
}

function writeTerminalOutput(data: Uint8Array) {
  for (const path of directoryParser.push(data)) {
    if (props.tab.sshFollowDirectory) void followTerminalDirectory(path);
  }
  terminal?.write(data);
}

function resetZmodemSentry() {
  zmodemSentry = createZmodemSentry({
    send(data) {
      if (terminalSocket?.readyState !== WebSocket.OPEN) {
        throw new Error(t("sshWorkbench.terminalDisconnected"));
      }
      terminalSocket.send(data.slice().buffer as ArrayBuffer);
    },
    toTerminal: writeTerminalOutput,
    onDetect: handleZmodemDetection,
    onRetract() {
      // Detection can be retracted when ordinary terminal bytes follow a partial signature.
    },
  });
}

function handleZmodemDetection(detection: ZmodemDetection) {
  if (pendingZmodemFiles.length === 0 || detection.get_session_role() !== "send") {
    detection.deny();
    if (pendingZmodemFiles.length > 0) finishZmodemUpload(new Error(t("sshWorkbench.zmodemUploadOnly")));
    return;
  }

  let session: ZmodemSession;
  try {
    session = detection.confirm();
  } catch (error) {
    finishZmodemUpload(error);
    return;
  }
  window.clearTimeout(zmodemDetectionTimer);
  zmodemSession = session;
  zmodemState.value = "uploading";
  zmodemSampledAt = performance.now();
  zmodemSampledBytes = 0;
  const files = pendingZmodemFiles;
  void sendZmodemFiles(session, files, updateZmodemProgress)
    .then(() => {
      toast(t("sshWorkbench.zmodemUploadComplete", { count: files.length }));
      finishZmodemUpload();
      void loadDirectory();
    })
    .catch((error) => finishZmodemUpload(error));
}

function updateZmodemProgress(progress: ZmodemUploadProgress) {
  zmodemFileName.value = progress.file.name;
  zmodemTransferred.value = progress.totalTransferred;
  zmodemTotalSize.value = progress.totalSize;
  const sampledAt = performance.now();
  if (sampledAt > zmodemSampledAt) {
    const elapsed = sampledAt - zmodemSampledAt;
    if (elapsed >= 250 || progress.totalTransferred === progress.totalSize) {
      const instantaneous = ((progress.totalTransferred - zmodemSampledBytes) * 1000) / elapsed;
      zmodemSpeed.value = zmodemSpeed.value > 0 ? zmodemSpeed.value * 0.65 + instantaneous * 0.35 : instantaneous;
      zmodemSampledAt = sampledAt;
      zmodemSampledBytes = progress.totalTransferred;
    }
  }
}

function finishZmodemUpload(error?: unknown) {
  window.clearTimeout(zmodemDetectionTimer);
  if (error && zmodemSession && !zmodemSession.has_ended()) {
    try {
      zmodemSession.abort();
    } catch {
      // The socket may already be closed.
    }
  }
  if (error) {
    const message = error instanceof Error ? error.message : String(error);
    toast(t("sshWorkbench.zmodemUploadFailed", { error: message }), 5000);
  }
  pendingZmodemFiles = [];
  zmodemSession = null;
  zmodemState.value = "idle";
  zmodemFileName.value = "";
  zmodemTransferred.value = 0;
  zmodemTotalSize.value = 0;
  zmodemSpeed.value = 0;
  resetZmodemSentry();
  terminal?.focus();
}

function decodeTerminalFrame(buffer: ArrayBuffer) {
  if (buffer.byteLength < 9 || !terminal) return;
  const view = new DataView(buffer);
  const sequence = Number(view.getBigUint64(0, false));
  const stream = view.getUint8(8);
  const data = new Uint8Array(buffer, 9);
  props.tab.sshLastSequence = sequence;
  if (stream === 2) {
    const stateMessage = data.length ? new TextDecoder().decode(data) : "";
    if (stateMessage === "directory-tracking-unavailable") {
      props.tab.sshFollowDirectory = false;
      props.tab.sshDirectoryTrackingSupported = false;
      toast(t("sshWorkbench.directoryTrackingUnavailable"), 3500);
      return;
    }
    window.clearTimeout(terminalReconnectTimer);
    terminalState.value = "disconnected";
    props.tab.sshConnected = false;
    terminalError.value = stateMessage === "ssh-transport-disconnected" || stateMessage === "disconnected" ? t("sshWorkbench.transportDisconnected") : stateMessage || t("sshWorkbench.disconnected");
    return;
  }
  try {
    if (!zmodemSentry) resetZmodemSentry();
    zmodemSentry?.consume(data.slice().buffer);
  } catch (error) {
    if (zmodemBusy.value) finishZmodemUpload(error);
    else {
      resetZmodemSentry();
      writeTerminalOutput(data);
    }
  }
}

async function attachTerminal(sessionId: string) {
  closeTerminalSocket();
  const socket = await api.sshConnectTerminal(sessionId, props.tab.sshLastSequence || 0);
  terminalSocket = socket;
  socket.binaryType = "arraybuffer";
  socket.onopen = () => {
    if (terminalSocket !== socket) return;
    terminalReconnectAttempt = 0;
    window.clearTimeout(terminalReconnectTimer);
    terminalState.value = "connected";
    terminalError.value = "";
    props.tab.sshConnected = true;
    resetZmodemSentry();
    scheduleFit();
    terminal?.focus();
    if (props.tab.sshFollowDirectory) {
      socket.send(JSON.stringify({ type: "directoryTracking", enabled: true }));
    }
  };
  socket.onmessage = (event) => {
    if (event.data instanceof ArrayBuffer) decodeTerminalFrame(event.data);
  };
  socket.onerror = () => {
    terminalError.value = t("sshWorkbench.socketFailed");
  };
  socket.onclose = () => {
    if (terminalSocket !== socket) return;
    terminalSocket = null;
    props.tab.sshConnected = false;
    if (zmodemBusy.value) finishZmodemUpload(new Error(t("sshWorkbench.terminalDisconnected")));
    if (
      shouldReattachTerminal({
        disposed,
        state: terminalState.value,
        expectedSessionId: sessionId,
        currentSessionId: props.tab.sshSessionId,
      })
    ) {
      scheduleTerminalReattach(sessionId);
    }
  };
}

function closeTerminalSocket() {
  const socket = terminalSocket;
  terminalSocket = null;
  if (!socket) return;
  socket.onopen = null;
  socket.onmessage = null;
  socket.onerror = null;
  socket.onclose = null;
  socket.close();
}

function scheduleTerminalReattach(sessionId: string) {
  window.clearTimeout(terminalReconnectTimer);
  const delay = terminalReconnectDelay(terminalReconnectAttempt);
  terminalReconnectAttempt += 1;
  terminalState.value = "connecting";
  terminalError.value = t("sshWorkbench.reattachingTerminal");
  terminalReconnectTimer = window.setTimeout(() => {
    if (
      !shouldReattachTerminal({
        disposed,
        state: terminalState.value,
        expectedSessionId: sessionId,
        currentSessionId: props.tab.sshSessionId,
      })
    )
      return;
    void attachTerminal(sessionId).catch(() => scheduleTerminalReattach(sessionId));
  }, delay);
}

async function connectSession(forceNew = false) {
  window.clearTimeout(terminalReconnectTimer);
  terminalReconnectAttempt = 0;
  props.tab.sshRestored = false;
  props.tab.sshConnected = false;
  terminalState.value = "connecting";
  terminalError.value = "";
  try {
    if (forceNew && props.tab.sshSessionId) {
      await api.sshCloseSession(props.tab.sshSessionId).catch(() => undefined);
      props.tab.sshSessionId = undefined;
      props.tab.sshLastSequence = 0;
      terminal?.clear();
    }
    createTerminal();
    if (!props.tab.sshSessionId) {
      const info = await api.sshCreateSession(props.connection, terminal?.cols || 120, terminal?.rows || 32);
      props.tab.sshSessionId = info.sessionId;
      props.tab.sshLastSequence = info.sequence;
      props.tab.sshDirectoryTrackingSupported = info.directoryTrackingSupported;
    }
    await attachTerminal(props.tab.sshSessionId);
    if (!currentPath.value) {
      try {
        currentPath.value = await api.sftpHome(props.tab.sshSessionId);
      } catch {
        try {
          entries.value = await api.sftpList(props.tab.sshSessionId, "/home");
          currentPath.value = "/home";
        } catch {
          currentPath.value = "/";
        }
      }
      props.tab.sshSftpPath = currentPath.value;
    }
    await loadDirectory(currentPath.value);
  } catch (error) {
    terminalState.value = "error";
    terminalError.value = error instanceof Error ? error.message : String(error);
  }
}

async function followTerminalDirectory(path: string) {
  const normalizedPath = normalizeRemotePath(path);
  if (normalizedPath === normalizeRemotePath(currentPath.value) || normalizedPath === pendingFollowPath.value || normalizedPath === failedFollowPath.value) {
    return;
  }
  pendingFollowPath.value = normalizedPath;
  await loadDirectory(normalizedPath, true);
}

async function loadDirectory(path = currentPath.value, fromTerminal = false) {
  if (!props.tab.sshSessionId) return;
  const normalizedPath = normalizeRemotePath(path || "/");
  const requestSequence = ++directoryRequestSequence;
  sftpLoading.value = true;
  if (!fromTerminal) sftpError.value = "";
  try {
    const nextEntries = await api.sftpList(props.tab.sshSessionId, normalizedPath);
    if (requestSequence !== directoryRequestSequence) return;
    entries.value = nextEntries;
    currentPath.value = normalizedPath;
    props.tab.sshSftpPath = currentPath.value;
    selectedSftpPath.value = "";
    failedFollowPath.value = "";
  } catch (error) {
    if (requestSequence !== directoryRequestSequence) return;
    const message = error instanceof Error ? error.message : String(error);
    if (fromTerminal) {
      failedFollowPath.value = normalizedPath;
      toast(t("sshWorkbench.followDirectoryFailed", { path: normalizedPath, error: message }), 5000);
    } else sftpError.value = message;
  } finally {
    if (pendingFollowPath.value === normalizedPath) pendingFollowPath.value = "";
    if (requestSequence === directoryRequestSequence) sftpLoading.value = false;
  }
}

function toggleDirectoryTracking(enabled: boolean) {
  if (enabled && props.tab.sshDirectoryTrackingSupported === false) {
    props.tab.sshFollowDirectory = false;
    toast(t("sshWorkbench.directoryTrackingUnsupported"), 4500);
    return;
  }
  if (pendingTerminalInput.value) {
    toast(t("sshWorkbench.followDirectoryInputPending"), 4000);
    return;
  }
  if (!terminalSocket || terminalSocket.readyState !== WebSocket.OPEN) {
    toast(t("sshWorkbench.terminalDisconnected"), 4000);
    return;
  }
  props.tab.sshFollowDirectory = enabled;
  directoryParser.reset();
  pendingFollowPath.value = "";
  failedFollowPath.value = "";
  terminalSocket.send(JSON.stringify({ type: "directoryTracking", enabled }));
  terminal?.focus();
}

function terminalMenuItems(): ContextMenuItem[] {
  return [
    {
      label: t("sshWorkbench.terminalCopy"),
      icon: Copy,
      disabled: () => !terminal?.hasSelection(),
      action: () => void copyTerminalSelection(),
    },
    {
      label: t("sshWorkbench.terminalPaste"),
      icon: ClipboardPaste,
      disabled: () => terminalState.value !== "connected" || zmodemBusy.value,
      action: () => void pasteIntoTerminal(),
    },
    {
      label: t("sshWorkbench.terminalSelectAll"),
      icon: TextSelect,
      action: selectAllTerminal,
    },
    {
      label: t("sshWorkbench.terminalClear"),
      icon: Eraser,
      action: clearTerminal,
    },
    { label: "", separator: true },
    {
      label: t("sshWorkbench.zmodemUpload"),
      icon: FileUp,
      disabled: () => terminalState.value !== "connected" || zmodemBusy.value || !canWrite.value,
      action: chooseZmodemFiles,
    },
  ];
}

async function copyTerminalSelection() {
  const selection = terminal?.getSelection() || "";
  if (!selection) return;
  try {
    await copyToClipboard(selection);
    toast(t("sshWorkbench.terminalCopied"));
  } catch (error) {
    toast(t("sshWorkbench.terminalCopyFailed", { error: error instanceof Error ? error.message : String(error) }), 5000);
  } finally {
    terminal?.focus();
  }
}

async function pasteIntoTerminal() {
  if (terminalState.value !== "connected" || zmodemBusy.value || terminalSocket?.readyState !== WebSocket.OPEN) return;
  try {
    const text = await readTextFromClipboard();
    if (text) terminalSocket.send(new TextEncoder().encode(text));
  } catch (error) {
    toast(t("sshWorkbench.terminalPasteFailed", { error: error instanceof Error ? error.message : String(error) }), 5000);
  } finally {
    terminal?.focus();
  }
}

function selectAllTerminal() {
  terminal?.selectAll();
  terminal?.focus();
}

function clearTerminal() {
  terminal?.clear();
  terminal?.focus();
}

function chooseZmodemFiles() {
  zmodemFileInput.value?.click();
}

function onZmodemFilesSelected(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files || []);
  input.value = "";
  if (files.length === 0 || terminalState.value !== "connected" || terminalSocket?.readyState !== WebSocket.OPEN) {
    terminal?.focus();
    return;
  }

  pendingZmodemFiles = files;
  zmodemState.value = "waiting";
  zmodemFileName.value = files[0]?.name || "";
  zmodemTransferred.value = 0;
  zmodemTotalSize.value = files.reduce((sum, file) => sum + file.size, 0);
  zmodemSpeed.value = 0;
  resetZmodemSentry();
  terminalSocket.send(new TextEncoder().encode("rz\r"));
  window.clearTimeout(zmodemDetectionTimer);
  zmodemDetectionTimer = window.setTimeout(() => {
    finishZmodemUpload(new Error(t("sshWorkbench.zmodemNotAvailable")));
  }, ZMODEM_DETECTION_TIMEOUT_MS);
}

async function openEntry(entry: SftpEntry) {
  if (entry.kind === "directory") {
    await loadDirectory(entry.path);
    return;
  }
  if (entry.kind !== "file" || !props.tab.sshSessionId) return;
  if (!isInlinePreviewSupported(entry)) {
    await downloadFile(entry);
    return;
  }
  previewOpen.value = true;
  previewLoading.value = true;
  previewTitle.value = entry.name;
  previewText.value = "";
  try {
    const preview = await api.sftpPreview(props.tab.sshSessionId, entry.path);
    const binary = atob(preview.base64);
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    previewText.value = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (/preview limit|exceeds the .*byte/i.test(message)) {
      previewOpen.value = false;
      await downloadFile(entry);
      return;
    }
    previewText.value = message;
  } finally {
    previewLoading.value = false;
  }
}

function showMkdir() {
  operationDraft.value = "";
  operationDialog.value = "mkdir";
}

async function startRename(entry: SftpEntry) {
  renamingPath.value = entry.path;
  renameDraft.value = entry.name;
  selectedSftpPath.value = entry.path;
  await nextTick();
  const element = (renameInput.value as any)?.$el as HTMLInputElement | undefined;
  element?.focus();
  element?.select();
}

function cancelRename() {
  renamingPath.value = "";
  renameDraft.value = "";
}

async function submitRename(entry: SftpEntry) {
  if (renamingPath.value !== entry.path || renameSubmitting.value || !props.tab.sshSessionId) return;
  const value = renameDraft.value.trim();
  if (!value || value === entry.name) {
    cancelRename();
    return;
  }
  renameSubmitting.value = true;
  const nextPath = joinRemotePath(currentPath.value, value);
  try {
    await api.sftpRename(props.tab.sshSessionId, entry.path, nextPath);
    renamingPath.value = "";
    renameDraft.value = "";
    await loadDirectory();
    selectedSftpPath.value = nextPath;
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
    await nextTick();
    const element = (renameInput.value as any)?.$el as HTMLInputElement | undefined;
    element?.focus();
    element?.select();
  } finally {
    renameSubmitting.value = false;
  }
}

async function submitOperation() {
  const value = operationDraft.value.trim();
  if (!value || !props.tab.sshSessionId) return;
  try {
    await api.sftpMkdir(props.tab.sshSessionId, joinRemotePath(currentPath.value, value));
    operationDialog.value = null;
    await loadDirectory();
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
  }
}

async function confirmDelete() {
  const entry = deleteTarget.value;
  if (!entry || !props.tab.sshSessionId || deleteSubmitting.value) return;
  deleteSubmitting.value = true;
  try {
    await api.sftpDelete(props.tab.sshSessionId, entry.path, entry.kind === "directory");
    deleteTarget.value = null;
    await loadDirectory();
    toast(t("sshWorkbench.deleted"));
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
  } finally {
    deleteSubmitting.value = false;
  }
}

async function uploadFile() {
  if (!props.tab.sshSessionId || !canWrite.value) return;
  try {
    const source = await api.pickSftpUploadFile();
    if (!source) return;
    await uploadSources([source]);
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
  }
}

async function uploadSources(sources: Array<string | File>) {
  if (!props.tab.sshSessionId || !canWrite.value || sources.length === 0) return;
  transferPopoverOpen.value = true;
  for (const source of sources) {
    const task = await api.sftpUpload(props.tab.sshSessionId, source, joinRemotePath(currentPath.value, localFileName(source)));
    transferTasks.value[task.taskId] = task;
  }
  await loadDirectory();
  toast(t("sshWorkbench.uploaded", { count: sources.length }));
}

function updateTransferTask(task: SftpTransferTask) {
  if (task.sessionId !== props.tab.sshSessionId) return;
  const isNewTask = !transferTasks.value[task.taskId];
  const sampledAt = performance.now();
  const previous = transferSamples.get(task.taskId);
  if (previous && task.transferred >= previous.transferred && sampledAt > previous.sampledAt) {
    const instantaneous = ((task.transferred - previous.transferred) * 1000) / (sampledAt - previous.sampledAt);
    const speed = previous.speed > 0 ? previous.speed * 0.65 + instantaneous * 0.35 : instantaneous;
    transferSamples.set(task.taskId, { transferred: task.transferred, sampledAt, speed });
    transferSpeeds.value[task.taskId] = speed;
  } else {
    transferSamples.set(task.taskId, { transferred: task.transferred, sampledAt, speed: previous?.speed ?? 0 });
  }
  transferTasks.value[task.taskId] = task;
  if (isNewTask && (task.status === "queued" || task.status === "running")) transferPopoverOpen.value = true;
}

async function cancelTransfer(task: SftpTransferTask) {
  try {
    await api.cancelSftpTransfer(task.taskId);
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
  }
}

function transferPercent(task: SftpTransferTask): number {
  if (task.size <= 0) return task.status === "completed" ? 100 : 0;
  return Math.min(100, Math.round((task.transferred / task.size) * 100));
}

function transferSpeed(task: SftpTransferTask): string {
  const speed = transferSpeeds.value[task.taskId] ?? 0;
  return speed > 0 && (task.status === "queued" || task.status === "running") ? `${formatObjectBrowserBytes(speed)}/s` : "";
}

async function onSftpDrop(event: Event) {
  const detail = (event as CustomEvent<{ tabId: string; paths?: string[]; files?: File[] }>).detail;
  if (detail.tabId !== props.tab.id) return;
  sftpDragActive.value = false;
  try {
    await uploadSources(detail.paths || detail.files || []);
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
  }
}

function onSftpDragState(event: Event) {
  const detail = (event as CustomEvent<{ tabId: string; active: boolean }>).detail;
  if (detail.tabId === props.tab.id) sftpDragActive.value = detail.active;
}

async function downloadFile(entry: SftpEntry) {
  if (!props.tab.sshSessionId || entry.kind !== "file") return;
  try {
    const task = await api.sftpDownload(props.tab.sshSessionId, entry.path, entry.name);
    if (task) {
      transferTasks.value[task.taskId] = task;
      transferPopoverOpen.value = true;
    }
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), 5000);
  }
}

function sftpMenuItems(entry: SftpEntry): ContextMenuItem[] {
  return [
    {
      label: entry.kind === "directory" ? t("sshWorkbench.openFolder") : t("sshWorkbench.preview"),
      icon: entry.kind === "directory" ? Folder : FileText,
      visible: entry.kind === "directory" || (entry.kind === "file" && isInlinePreviewSupported(entry)),
      action: () => void openEntry(entry),
    },
    {
      label: t("sshWorkbench.download"),
      icon: Download,
      visible: entry.kind === "file",
      action: () => void downloadFile(entry),
    },
    { label: "", separator: true },
    {
      label: t("sshWorkbench.rename"),
      icon: Pencil,
      disabled: !canWrite.value,
      action: () => void startRename(entry),
    },
    {
      label: t("sshWorkbench.delete"),
      icon: Trash2,
      iconClass: "text-destructive",
      variant: "destructive",
      disabled: !canWrite.value,
      action: () => {
        deleteTarget.value = entry;
      },
    },
    { label: "", separator: true },
    {
      label: t("sshWorkbench.refresh"),
      icon: RefreshCw,
      shortcut: "F5",
      action: () => void loadDirectory(),
    },
  ];
}

watch(
  () => [editorSettings.value.fontFamily, editorSettings.value.fontSize] as const,
  ([fontFamily, fontSize]) => {
    if (!terminal) return;
    terminal.options.fontFamily = fontFamily;
    terminal.options.fontSize = fontSize;
    scheduleFit();
  },
);

watch(
  () => [isDark.value, themePalette.value] as const,
  async () => {
    await nextTick();
    if (terminal) terminal.options.theme = resolveTerminalTheme();
  },
);

watch(
  () => props.tab.sshConnectRequestId,
  (requestId, previousRequestId) => {
    if (!mounted || !requestId || requestId === previousRequestId || props.tab.sshConnected === true) return;
    void connectSession(true);
  },
);

onMounted(async () => {
  mounted = true;
  window.addEventListener("dbx:ssh-sftp-drop", onSftpDrop);
  window.addEventListener("dbx:ssh-sftp-drag-state", onSftpDragState);
  unlistenTransferProgress = await api.listenSftpTransferProgress(updateTransferTask);
  await nextTick();
  createTerminal();
  if (props.tab.sshRestored && !props.tab.sshSessionId) {
    terminalState.value = "disconnected";
    terminalError.value = t("sshWorkbench.restartDisconnected");
    return;
  }
  await connectSession(!!props.tab.sshSessionId && props.tab.sshConnected === false);
});

onBeforeUnmount(() => {
  window.removeEventListener("dbx:ssh-sftp-drop", onSftpDrop);
  window.removeEventListener("dbx:ssh-sftp-drag-state", onSftpDragState);
  unlistenTransferProgress?.();
  disposed = true;
  mounted = false;
  window.clearTimeout(resizeTimer);
  window.clearTimeout(terminalReconnectTimer);
  window.clearTimeout(zmodemDetectionTimer);
  if (zmodemSession && !zmodemSession.has_ended()) {
    try {
      zmodemSession.abort();
    } catch {
      // The terminal socket is already being disposed.
    }
  }
  resizeObserver?.disconnect();
  closeTerminalSocket();
  terminal?.dispose();
});
</script>

<template>
  <div class="ssh-workbench h-full min-h-0 bg-background" :style="{ '--ssh-terminal-background': terminalBackground }">
    <header class="workbench-toolbar" :style="toolbarStyle">
      <div class="header-title">
        <span v-if="connection.color" class="h-4 w-1 shrink-0 rounded-full" :style="{ backgroundColor: connection.color }" />
        <span class="truncate">{{ connectionIdentity }}</span>
      </div>
      <div class="flex min-w-0 items-center gap-0.5">
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon" class="h-6 w-6 text-muted-foreground hover:bg-accent hover:text-foreground" @click="togglePaneOrder">
              <ArrowLeftRight class="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{{ paneOrderToggleLabel }}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon" class="h-6 w-6 text-emerald-600 hover:bg-emerald-500/10 hover:text-emerald-700 dark:text-emerald-300 dark:hover:text-emerald-200" @click="connectSession(true)">
              <RefreshCw class="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{{ t("sshWorkbench.reconnect") }}</TooltipContent>
        </Tooltip>
        <div class="follow-directory-control">
          <Switch size="sm" :model-value="!!tab.sshFollowDirectory" :disabled="terminalState !== 'connected'" @update:model-value="toggleDirectoryTracking" />
          <span>{{ t("sshWorkbench.followTerminal") }}</span>
        </div>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon" class="h-6 w-6 text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-300 dark:hover:text-amber-200" @click="props.tab.sshSessionId && api.sftpHome(props.tab.sshSessionId).then(loadDirectory)">
              <Home class="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{{ t("sshWorkbench.home") }}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon" class="h-6 w-6 text-cyan-600 hover:bg-cyan-500/10 hover:text-cyan-700 dark:text-cyan-300 dark:hover:text-cyan-200" @click="loadDirectory()">
              <RefreshCw class="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{{ t("sshWorkbench.refresh") }}</TooltipContent>
        </Tooltip>
        <DropdownMenu>
          <LightTooltip :text="t('sshWorkbench.customizeColumns')">
            <DropdownMenuTrigger as-child>
              <Button variant="ghost" size="icon" class="h-6 w-6 text-violet-600 hover:bg-violet-500/10 hover:text-violet-700 dark:text-violet-300 dark:hover:text-violet-200">
                <Columns3 class="h-3.5 w-3.5" />
              </Button>
            </DropdownMenuTrigger>
          </LightTooltip>
          <DropdownMenuContent align="end" class="w-40">
            <DropdownMenuCheckboxItem indicator-position="left" :model-value="visibleSftpColumns.includes('size')" @select="(event: Event) => toggleSftpColumnFromMenu(event, 'size')">
              <template #indicator-icon><span class="h-2 w-2 rounded-full bg-emerald-500" /></template>
              {{ t("sshWorkbench.size") }}
            </DropdownMenuCheckboxItem>
            <DropdownMenuCheckboxItem indicator-position="left" :model-value="visibleSftpColumns.includes('modified')" @select="(event: Event) => toggleSftpColumnFromMenu(event, 'modified')">
              <template #indicator-icon><span class="h-2 w-2 rounded-full bg-emerald-500" /></template>
              {{ t("sshWorkbench.modified") }}
            </DropdownMenuCheckboxItem>
            <DropdownMenuCheckboxItem indicator-position="left" :model-value="visibleSftpColumns.includes('permissions')" @select="(event: Event) => toggleSftpColumnFromMenu(event, 'permissions')">
              <template #indicator-icon><span class="h-2 w-2 rounded-full bg-emerald-500" /></template>
              {{ t("sshWorkbench.permissions") }}
            </DropdownMenuCheckboxItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Popover v-model:open="transferPopoverOpen">
          <LightTooltip :text="t('sshWorkbench.transfers')">
            <PopoverTrigger as-child>
              <Button variant="ghost" size="icon" class="relative h-6 w-6 text-blue-600 hover:bg-blue-500/10 hover:text-blue-700 dark:text-blue-300 dark:hover:text-blue-200">
                <ListChecks class="h-3.5 w-3.5" />
                <span v-if="Object.values(transferTasks).some((task) => task.status === 'queued' || task.status === 'running')" class="absolute right-0.5 top-0.5 h-1.5 w-1.5 rounded-full bg-primary" />
              </Button>
            </PopoverTrigger>
          </LightTooltip>
          <PopoverContent align="end" class="w-80 p-2">
            <div class="mb-2 px-1 text-xs font-semibold">{{ t("sshWorkbench.transfers") }}</div>
            <div v-if="Object.keys(transferTasks).length === 0" class="px-2 py-8 text-center text-xs text-muted-foreground">{{ t("sshWorkbench.noTransfers") }}</div>
            <div v-else class="max-h-72 space-y-1 overflow-auto">
              <div v-for="task in Object.values(transferTasks).slice().reverse()" :key="task.taskId" class="rounded border p-2">
                <div class="flex items-center gap-2">
                  <FileUp v-if="task.direction === 'upload'" class="h-3.5 w-3.5 shrink-0 text-teal-600 dark:text-teal-300" />
                  <FileDown v-else class="h-3.5 w-3.5 shrink-0 text-sky-600 dark:text-sky-300" />
                  <span class="min-w-0 flex-1 truncate text-xs">{{ task.fileName }}</span>
                  <span class="rounded bg-muted px-1.5 py-0.5 text-[9px] text-muted-foreground">
                    {{ task.direction === "upload" ? t("sshWorkbench.upload") : t("sshWorkbench.download") }}
                  </span>
                  <span class="text-[10px] text-muted-foreground">{{ transferPercent(task) }}%</span>
                  <Button v-if="task.status === 'queued' || task.status === 'running'" size="icon-xs" variant="ghost" @click="cancelTransfer(task)"><X class="h-3 w-3" /></Button>
                </div>
                <div class="mt-1.5 h-1 overflow-hidden rounded bg-muted">
                  <div class="h-full bg-primary transition-[width]" :style="{ width: `${transferPercent(task)}%` }" />
                </div>
                <div class="mt-1 grid grid-cols-[minmax(64px,1fr)_76px_auto] items-center gap-2 text-[10px] text-muted-foreground">
                  <span class="truncate">{{ t(`sshWorkbench.transferStatus.${task.status}`) }}</span>
                  <span class="w-[76px] text-left tabular-nums">{{ transferSpeed(task) || "\u00a0" }}</span>
                  <span class="whitespace-nowrap text-right tabular-nums">{{ formatObjectBrowserBytes(task.transferred) }} / {{ formatObjectBrowserBytes(task.size) }}</span>
                </div>
              </div>
            </div>
          </PopoverContent>
        </Popover>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon" class="h-6 w-6 text-teal-600 hover:bg-teal-500/10 hover:text-teal-700 dark:text-teal-300 dark:hover:text-teal-200" :disabled="!canWrite" @click="uploadFile">
              <FileUp class="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{{ t("sshWorkbench.upload") }}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon" class="h-6 w-6 text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-300 dark:hover:text-amber-200" :disabled="!canWrite" @click="showMkdir">
              <FolderPlus class="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{{ t("sshWorkbench.newFolder") }}</TooltipContent>
        </Tooltip>
      </div>
    </header>

    <Splitpanes :rtl="splitLayout.rtl" class="min-h-0 flex-1 overflow-hidden" :style="{ flexDirection: splitLayout.flexDirection }" @resize="onSplitResize">
      <Pane :size="splitRatio" :min-size="35">
        <section class="terminal-pane">
          <CustomContextMenu :items="terminalMenuItems" v-slot="{ onContextMenu }">
            <div class="terminal-body" @contextmenu="onContextMenu">
              <div ref="terminalHost" class="terminal-host" />
              <div v-if="terminalState !== 'connected'" class="terminal-state">
                <Loader2 v-if="terminalState === 'connecting'" class="h-7 w-7 animate-spin" />
                <DatabaseIcon v-else db-type="ssh" class="h-9 w-9 opacity-60" />
                <p>{{ terminalState === "connecting" ? t("sshWorkbench.connecting") : terminalError || t("sshWorkbench.disconnected") }}</p>
                <Button v-if="terminalState !== 'connecting'" size="sm" @click="connectSession(true)">{{ t("sshWorkbench.reconnect") }}</Button>
              </div>
              <div v-if="zmodemBusy" class="zmodem-status">
                <Loader2 class="h-3.5 w-3.5 shrink-0 animate-spin" />
                <span class="min-w-0 flex-1 truncate">
                  {{ zmodemState === "waiting" ? t("sshWorkbench.zmodemWaiting") : t("sshWorkbench.zmodemUploading", { name: zmodemFileName, percent: zmodemPercent }) }}
                </span>
                <span v-if="zmodemState === 'uploading'" class="shrink-0 tabular-nums text-muted-foreground">
                  {{ zmodemSpeed > 0 ? `${formatObjectBrowserBytes(zmodemSpeed)}/s` : "\u00a0" }}
                </span>
              </div>
            </div>
          </CustomContextMenu>
        </section>
      </Pane>

      <Pane :size="100 - splitRatio" :min-size="20">
        <section class="sftp-pane" :class="{ 'sftp-drag-active': sftpDragActive }" @dragenter.prevent="sftpDragActive = true" @dragover.prevent="sftpDragActive = true" @dragleave.self="sftpDragActive = false" @drop.prevent="sftpDragActive = false">
          <div class="path-row">
            <Tooltip>
              <TooltipTrigger as-child>
                <Button variant="ghost" size="icon" class="h-6 w-6 text-muted-foreground hover:bg-accent hover:text-foreground" @click="loadDirectory(parentPath(currentPath))">
                  <ArrowUp class="h-3.5 w-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{{ t("sshWorkbench.parentFolder") }}</TooltipContent>
            </Tooltip>
            <Input v-model="currentPath" class="h-7 min-w-0 flex-1 font-mono text-xs" @keydown.enter="loadDirectory(currentPath)" />
          </div>
          <div class="sftp-table">
            <div class="sftp-grid sftp-grid-header" :style="sftpGridStyle">
              <button type="button" class="sftp-sort-button" @click="toggleSftpSort('name')">
                <span>{{ t("sshWorkbench.name") }}</span>
                <component :is="sortIcon('name')" class="h-3 w-3" />
              </button>
              <button v-if="visibleSftpColumns.includes('size')" type="button" class="sftp-sort-button" @click="toggleSftpSort('size')">
                <span>{{ t("sshWorkbench.size") }}</span>
                <component :is="sortIcon('size')" class="h-3 w-3" />
              </button>
              <button v-if="visibleSftpColumns.includes('modified')" type="button" class="sftp-sort-button" @click="toggleSftpSort('modified')">
                <span>{{ t("sshWorkbench.modified") }}</span>
                <component :is="sortIcon('modified')" class="h-3 w-3" />
              </button>
              <span v-if="visibleSftpColumns.includes('permissions')">{{ t("sshWorkbench.permissions") }}</span>
            </div>
            <div v-if="sftpLoading" class="sftp-empty"><Loader2 class="h-5 w-5 animate-spin" /> {{ t("sshWorkbench.loading") }}</div>
            <div v-else-if="sftpError" class="sftp-empty text-destructive">{{ sftpError }}</div>
            <div v-else-if="entries.length === 0" class="sftp-empty">{{ t("sshWorkbench.emptyFolder") }}</div>
            <CustomContextMenu v-for="entry in sortedEntries" v-else :key="entry.path" :items="sftpMenuItems(entry)" v-slot="{ onContextMenu }">
              <div
                class="sftp-grid sftp-row"
                :class="{ 'sftp-row-selected': selectedSftpPath === entry.path }"
                :style="sftpGridStyle"
                @click="selectedSftpPath = entry.path"
                @dblclick="openEntry(entry)"
                @contextmenu="
                  (event) => {
                    selectedSftpPath = entry.path;
                    onContextMenu(event);
                  }
                "
              >
                <div v-if="renamingPath === entry.path" class="flex min-w-0 items-center gap-2">
                  <Folder v-if="entry.kind === 'directory'" class="h-4 w-4 shrink-0 text-amber-500" />
                  <FileText v-else-if="entry.kind === 'file'" class="h-4 w-4 shrink-0 text-sky-500" />
                  <File v-else class="h-4 w-4 shrink-0 text-muted-foreground" />
                  <Input ref="renameInput" v-model="renameDraft" class="h-6 min-w-0 flex-1 px-1.5 text-xs" :disabled="renameSubmitting" @click.stop @dblclick.stop @keydown.enter.prevent.stop="submitRename(entry)" @keydown.esc.prevent.stop="cancelRename" @blur="submitRename(entry)" />
                </div>
                <LightTooltip v-else :text="entry.name">
                  <div class="flex min-w-0 items-center gap-2">
                    <Folder v-if="entry.kind === 'directory'" class="h-4 w-4 shrink-0 text-amber-500" />
                    <FileText v-else-if="entry.kind === 'file'" class="h-4 w-4 shrink-0 text-sky-500" />
                    <File v-else class="h-4 w-4 shrink-0 text-muted-foreground" />
                    <span class="truncate">{{ entry.name }}</span>
                  </div>
                </LightTooltip>
                <span v-if="visibleSftpColumns.includes('size')" class="truncate tabular-nums text-muted-foreground">{{ entry.kind === "file" ? formatObjectBrowserBytes(entry.size) : "" }}</span>
                <span v-if="visibleSftpColumns.includes('modified')" class="truncate tabular-nums text-muted-foreground">{{ formattedModifiedAt(entry.modifiedAt) }}</span>
                <span v-if="visibleSftpColumns.includes('permissions')" class="truncate font-mono text-muted-foreground">{{ entry.permissions || "" }}</span>
              </div>
            </CustomContextMenu>
          </div>
          <footer class="sftp-footer">{{ t("sshWorkbench.items", { count: entries.length }) }}</footer>
        </section>
      </Pane>
    </Splitpanes>
    <input ref="zmodemFileInput" class="hidden" type="file" multiple @change="onZmodemFilesSelected" />

    <Dialog :open="operationDialog === 'mkdir'" @update:open="(open) => !open && (operationDialog = null)">
      <DialogContent class="sm:max-w-[420px]">
        <DialogHeader
          ><DialogTitle>{{ t("sshWorkbench.newFolder") }}</DialogTitle></DialogHeader
        >
        <Input v-model="operationDraft" autofocus @keydown.enter="submitOperation" />
        <DialogFooter>
          <Button variant="outline" @click="operationDialog = null">{{ t("common.cancel") }}</Button>
          <Button :disabled="!operationDraft.trim()" @click="submitOperation">{{ t("sshWorkbench.confirm") }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <Dialog v-model:open="previewOpen">
      <DialogContent class="sm:max-w-[760px]">
        <DialogHeader
          ><DialogTitle class="truncate">{{ previewTitle }}</DialogTitle></DialogHeader
        >
        <div v-if="previewLoading" class="flex h-72 items-center justify-center"><Loader2 class="h-6 w-6 animate-spin" /></div>
        <SshTextPreview v-else :text="previewText" />
      </DialogContent>
    </Dialog>

    <DangerConfirmDialog
      :open="!!deleteTarget"
      :title="t('sshWorkbench.deleteTitle')"
      :message="t('sshWorkbench.deleteMessage')"
      :details="deleteTarget?.path || ''"
      :confirm-label="t('sshWorkbench.delete')"
      :loading="deleteSubmitting"
      :close-on-confirm="false"
      @update:open="(open) => !open && (deleteTarget = null)"
      @confirm="confirmDelete"
    />
  </div>
</template>

<style scoped>
.ssh-workbench {
  display: flex;
  flex-direction: column;
}

.terminal-pane,
.sftp-pane {
  display: flex;
  height: 100%;
  min-width: 0;
  flex-direction: column;
  background: var(--background);
}

.sftp-pane {
  position: relative;
}

.sftp-drag-active {
  background: color-mix(in srgb, var(--primary) 4%, var(--background));
}

.sftp-drag-active::after {
  position: absolute;
  z-index: 20;
  inset: 4px;
  border: 2px dashed var(--primary);
  border-radius: var(--radius);
  background: color-mix(in srgb, var(--primary) 9%, transparent);
  content: "";
  pointer-events: none;
}

.workbench-toolbar {
  display: flex;
  height: 36px;
  flex: 0 0 36px;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  border-bottom: 1px solid var(--border);
  padding: 0 8px;
  background: color-mix(in srgb, var(--background) 92%, transparent);
}

.header-title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
}

.terminal-body {
  position: relative;
  min-height: 0;
  flex: 1;
  overflow: hidden;
  background: var(--ssh-terminal-background);
}

.terminal-host {
  position: absolute;
  inset: 0;
  padding: 6px 0 6px 8px;
}

.terminal-state {
  position: absolute;
  inset: 0;
  z-index: 2;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-direction: column;
  gap: 10px;
  padding: 24px;
  background: var(--ssh-terminal-background);
  color: var(--muted-foreground);
  text-align: center;
  font-size: 12px;
}

.zmodem-status {
  position: absolute;
  z-index: 3;
  right: 8px;
  bottom: 8px;
  left: 8px;
  display: flex;
  align-items: center;
  gap: 7px;
  border: 1px solid color-mix(in srgb, var(--primary) 40%, var(--border));
  border-radius: var(--radius);
  padding: 6px 8px;
  background: color-mix(in srgb, var(--background) 92%, transparent);
  box-shadow: 0 4px 14px color-mix(in srgb, #000 18%, transparent);
  color: var(--foreground);
  font-size: 11px;
  pointer-events: none;
}

.path-row {
  display: flex;
  flex: 0 0 38px;
  align-items: center;
  gap: 4px;
  border-bottom: 1px solid var(--border);
  padding: 4px 7px;
}

.sftp-table {
  min-height: 0;
  flex: 1;
  overflow: auto;
}

.sftp-grid {
  display: grid;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
}

.sftp-grid-header {
  position: sticky;
  top: 0;
  z-index: 3;
  height: 28px;
  border-bottom: 1px solid var(--border);
  background: var(--background);
  box-shadow: 0 1px 2px color-mix(in srgb, var(--foreground) 8%, transparent);
  color: var(--muted-foreground);
  font-size: 11px;
}

.sftp-sort-button {
  display: flex;
  min-width: 0;
  height: 100%;
  align-items: center;
  justify-content: space-between;
  gap: 3px;
  border-radius: 3px;
  color: inherit;
  cursor: pointer;
}

.sftp-sort-button:hover {
  color: var(--foreground);
}

.sftp-row {
  height: 30px;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 45%, transparent);
  font-size: 12px;
  cursor: default;
}

.sftp-row:hover {
  background: color-mix(in srgb, var(--accent) 65%, transparent);
}

.sftp-row-selected,
.sftp-row-selected:hover {
  background: var(--accent);
  color: var(--accent-foreground);
}

.follow-directory-control {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-right: 2px;
  color: var(--muted-foreground);
  font-size: 11px;
  white-space: nowrap;
}

.sftp-empty {
  display: flex;
  height: 120px;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 16px;
  color: var(--muted-foreground);
  font-size: 12px;
  text-align: center;
}

.sftp-footer {
  display: flex;
  height: 26px;
  flex: 0 0 26px;
  align-items: center;
  border-top: 1px solid var(--border);
  padding: 0 9px;
  color: var(--muted-foreground);
  font-size: 11px;
}

:deep(.xterm-viewport) {
  scrollbar-width: thin;
  scrollbar-color: #536271 transparent;
}
</style>
