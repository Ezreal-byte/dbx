<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { useCellDetailEditor } from "@/composables/useCellDetailEditor";
import { useSettingsStore } from "@/stores/settingsStore";
import { useTheme } from "@/composables/useTheme";

const props = defineProps<{ text: string }>();
const host = ref<HTMLElement | null>(null);
const settingsStore = useSettingsStore();
const { isDark, themePalette } = useTheme();
const editor = useCellDetailEditor({
  language: "auto",
  readOnly: true,
  lineNumbers: true,
  editorTheme: () => settingsStore.editorSettings.theme,
  appAppearance: () => (isDark.value ? "dark" : "light"),
  appPalette: () => themePalette.value,
  fontSize: () => settingsStore.editorSettings.fontSize,
  fontFamily: () => settingsStore.editorSettings.fontFamily,
});

onMounted(async () => {
  if (host.value) await editor.create(host.value, props.text, "text");
});

watch(
  () => props.text,
  (value) => editor.setValue(value, "text"),
);
</script>

<template>
  <div ref="host" class="h-[65vh] min-h-0 overflow-hidden rounded border" />
</template>
