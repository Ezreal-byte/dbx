<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Button } from "@/components/ui/button";
import LightTooltip from "@/components/ui/LightTooltip.vue";
import PluginIcon from "./PluginIcon.vue";
import { usePluginShortcuts } from "@/composables/usePluginShortcuts";
import { usePluginShortcutPreferences } from "@/composables/usePluginShortcutPreferences";
import { usePluginShortcutSort } from "@/composables/usePluginShortcutSort";
import { usePluginShortcutHeight } from "@/composables/usePluginShortcutHeight";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToast } from "@/composables/useToast";
import { movePluginShortcut, type PluginShortcutEntry, type PluginShortcutPosition } from "@/lib/plugins/pluginShortcuts";

const props = defineProps<{ position: PluginShortcutPosition }>();
const { t } = useI18n();
const settings = useSettingsStore();
const { toast } = useToast();
const { entries, open, isActive } = usePluginShortcuts();
const { update, saving } = usePluginShortcutPreferences();
const section = ref<HTMLElement | null>(null);
const scroll = ref<HTMLElement | null>(null);
const horizontal = computed(() => props.position === "sidebar-bottom");
const left = computed(() => props.position.startsWith("left-"));
const bottom = computed(() => !horizontal.value && props.position.endsWith("-bottom"));
const ids = computed(() => entries.value.map((entry) => entry.id));
function saveError(error: unknown) {
  toast(`${t("pluginPlatform.shortcutsSaveFailed")}: ${String(error)}`, 5000);
}
const sizing = usePluginShortcutHeight({
  section,
  scroll,
  horizontal,
  count: computed(() => entries.value.length),
  savedHeight: computed(() => settings.editorSettings.pluginShortcuts.sidebarHeight),
  save: async (sidebarHeight) => {
    try {
      await update({ sidebarHeight });
    } catch (error) {
      saveError(error);
    }
  },
});
const sorting = usePluginShortcutSort({
  container: scroll,
  ids,
  horizontal,
  disabled: computed(() => saving.value || sizing.resizing.value),
  commit: async (source, target, after) => {
    try {
      await update((current) => ({ order: movePluginShortcut(current.order, ids.value, source, target, after) }));
    } catch (error) {
      saveError(error);
    }
  },
});
const { drag } = sorting;
const draggedEntry = computed(() => entries.value.find((entry) => entry.id === drag.source));
watch(() => props.position, sorting.cancel);
function activate(entry: PluginShortcutEntry) {
  if (sorting.suppressClick()) return;
  try {
    open(entry);
  } catch (error) {
    toast(String(error), 5000);
  }
}
</script>

<template>
  <nav
    v-if="settings.isEditorSettingsLoaded && settings.editorSettings.pluginShortcuts.enabled && entries.length"
    ref="section"
    :aria-label="t('pluginPlatform.shortcutsTitle')"
    :data-plugin-shortcut-position="position"
    class="plugin-shortcut-bar flex shrink-0 flex-col bg-muted/30"
    :class="horizontal ? 'min-w-0' : ['min-h-0 w-10 self-stretch', left ? 'border-r' : 'border-l']"
    @dragstart.prevent
  >
    <div
      v-if="horizontal"
      data-shortcut-resize
      role="separator"
      tabindex="0"
      aria-orientation="horizontal"
      :aria-label="t('pluginPlatform.shortcutsResize')"
      :aria-valuenow="Math.round(sizing.height.value)"
      :title="t('pluginPlatform.shortcutsResize')"
      class="h-1.5 shrink-0 cursor-row-resize touch-none border-t border-border transition-colors hover:bg-primary/30 focus-visible:bg-primary/30 focus-visible:outline-none"
      @pointerdown="!saving && sizing.start($event)"
      @dblclick="!saving && sizing.reset()"
      @keydown="!saving && sizing.resizeWithKeyboard($event)"
    />
    <div ref="scroll" data-shortcut-scroll class="min-h-0 overflow-y-auto overflow-x-hidden p-1" :class="horizontal ? 'shrink-0' : 'flex-1'" :style="horizontal ? { height: `${sizing.height.value}px` } : undefined">
      <div class="shortcut-items flex gap-1" :class="horizontal ? 'flex-row flex-wrap content-start' : ['min-h-full flex-col', { 'shortcut-items--bottom': bottom }]">
        <LightTooltip v-for="(entry, index) in entries" :key="entry.id" :text="entry.label" :disabled="drag.active || sizing.resizing.value" :side="horizontal ? 'top' : left ? 'right' : 'left'" content-class="shortcut-tooltip">
          <span
            class="shortcut-item relative flex shrink-0 touch-none"
            :class="{ 'mt-auto': bottom && index === 0, 'drop-before': drag.target === entry.id && !drag.after && drag.source !== entry.id, 'drop-after': drag.target === entry.id && drag.after && drag.source !== entry.id, 'opacity-40': drag.active && drag.source === entry.id }"
            :data-shortcut-id="entry.id"
            @pointerdown="sorting.start($event, entry.id)"
          >
            <Button variant="ghost" size="icon" class="shortcut-button relative size-8 shrink-0" :class="{ 'shortcut-button--active': isActive(entry) }" :aria-label="entry.label" :aria-pressed="isActive(entry)" :disabled="entry.disabled" @click="activate(entry)">
              <PluginIcon :plugin-id="entry.pluginId" :icon="entry.icon" class="size-4 [&_svg]:text-current [&_img]:pointer-events-none" />
              <span v-if="isActive(entry)" class="absolute rounded-full bg-primary" :class="horizontal ? 'bottom-0 h-0.5 w-3' : 'left-0 h-3 w-0.5'" aria-hidden="true" />
            </Button>
          </span>
        </LightTooltip>
      </div>
    </div>
    <Teleport to="body">
      <div v-if="drag.active || sizing.resizing.value" class="shortcut-pointer-overlay fixed inset-0 select-none touch-none" :style="{ cursor: drag.active ? 'grabbing' : 'row-resize' }" aria-hidden="true" />
      <div
        v-if="drag.active && draggedEntry"
        class="shortcut-drag-preview pointer-events-none fixed flex max-w-xs items-center gap-2 rounded-md border border-primary/50 bg-popover px-2 py-1.5 text-xs text-popover-foreground shadow-md"
        :style="{ left: `${drag.x + 12}px`, top: `${drag.y + 12}px` }"
        aria-hidden="true"
      >
        <PluginIcon :plugin-id="draggedEntry.pluginId" :icon="draggedEntry.icon" class="size-4 [&_svg]:text-current" />
        <span class="truncate">{{ draggedEntry.label }}</span>
      </div>
    </Teleport>
  </nav>
</template>

<style scoped>
/* The first actual flex item uses auto margin, keeping overflowing rails reachable. */
.shortcut-button {
  border: 1px solid transparent;
  color: var(--muted-foreground);
  transition:
    background-color 120ms,
    color 120ms,
    border-color 120ms;
}
.shortcut-button:hover {
  background: color-mix(in srgb, var(--primary) 14%, var(--background));
  border-color: color-mix(in srgb, var(--primary) 40%, transparent);
  color: var(--foreground);
}
.shortcut-button--active {
  background: var(--accent);
  color: var(--accent-foreground);
  border-color: color-mix(in srgb, var(--primary) 35%, transparent);
}
.shortcut-button:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: -2px;
}
.shortcut-item.drop-before::before,
.shortcut-item.drop-after::after {
  content: "";
  position: absolute;
  z-index: 1;
  pointer-events: none;
  background: var(--primary);
  height: 2px;
  left: 0;
  right: 0;
}
.shortcut-item.drop-before::before {
  top: -3px;
}
.shortcut-item.drop-after::after {
  bottom: -3px;
}
[data-plugin-shortcut-position="sidebar-bottom"] .drop-before::before,
[data-plugin-shortcut-position="sidebar-bottom"] .drop-after::after {
  width: 2px;
  height: auto;
  top: 0;
  bottom: 0;
}
[data-plugin-shortcut-position="sidebar-bottom"] .drop-before::before {
  left: -3px;
  right: auto;
}
[data-plugin-shortcut-position="sidebar-bottom"] .drop-after::after {
  right: -3px;
  left: auto;
}
.shortcut-pointer-overlay {
  z-index: 2147483646;
}
.shortcut-drag-preview {
  z-index: 2147483647;
}
</style>

<style>
.shortcut-tooltip {
  width: max-content;
  max-width: calc(100vw - 24px);
  white-space: normal;
  overflow-wrap: anywhere;
}
</style>
