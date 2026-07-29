import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en";
import es from "@/i18n/locales/es";
import itLocale from "@/i18n/locales/it";
import ja from "@/i18n/locales/ja";
import ptBR from "@/i18n/locales/pt-BR";
import zhCN from "@/i18n/locales/zh-CN";
import zhTW from "@/i18n/locales/zh-TW";

function leafKeys(value: unknown, prefix = ""): string[] {
  if (!value || typeof value !== "object") return [prefix];
  return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) => leafKeys(child, prefix ? `${prefix}.${key}` : key));
}

describe("SSH workbench translations", () => {
  it("keeps all seven locales structurally aligned", () => {
    const expected = leafKeys(en.sshWorkbench).sort();
    for (const locale of [zhCN, zhTW, es, itLocale, ja, ptBR]) {
      expect(leafKeys(locale.sshWorkbench).sort()).toEqual(expected);
    }
  });
});
