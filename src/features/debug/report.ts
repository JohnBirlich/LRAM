import { save as saveFile } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { toast, toastError } from "../../ui/Toast";
import { t } from "../../i18n";

// -- Main func
export async function saveReport(): Promise<void> {
  try {
    const path = await saveFile({
      defaultPath: `LRAM-diagnostics-${new Date().toISOString().slice(0, 10)}.txt`,
      filters: [{ name: t("debug.reportFilter"), extensions: ["txt"] }],
    });
    if (!path) return;
    await api.diagnosticsSave(path);
    toast(t("debug.reportSaved"));
  } catch (e) {
    toastError(e);
  }
}
