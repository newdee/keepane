import { Button, toast } from "@heroui/react";
import { Copy, Paperclip, X } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { getJson, key, needsCode, q } from "../api";
import { copyText } from "../copy";
import { t } from "../i18n";

/** Where a file sent for the pane goes and how big it may be
 *  (`/api/upload-info`), or why none can be sent (another machine's pane). */
type Place = { dir: string; max: number } | { error: string };
type Sent = { name: string; path: string };
type Going = { name: string; done: number; total: number; nth: number; of: number };

const size = (n: number) =>
  n >= 1 << 30 ? `${(n / (1 << 30)).toFixed(1)} GB` : n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`;

/** One file, sent as the body of `POST /api/upload` (XHR, for its progress):
 *  where it was written, or keepane's words why not. */
function sendOne(pane: string, file: File, onProgress: (done: number, total: number) => void): Promise<Sent> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `/api/upload?pane=${q(pane)}&name=${q(file.name)}`);
    xhr.setRequestHeader("X-Keepane-Key", key);
    xhr.upload.onprogress = (e) => e.lengthComputable && onProgress(e.loaded, e.total);
    xhr.onload = () => {
      if (xhr.status === 200) {
        try {
          const v = JSON.parse(xhr.responseText) as { name: string; path: string };
          return resolve({ name: v.name, path: v.path });
        } catch {
          return reject(new Error(xhr.responseText));
        }
      }
      reject(new Error(xhr.status === 401 ? needsCode() : xhr.responseText || xhr.statusText));
    };
    xhr.onerror = () => reject(new Error(t("The upload broke off", "上传中断了")));
    xhr.send(file);
  });
}

/** Files from this device into the pane's folder (`web-upload-dir`, under
 *  its directory): the 📎 button for the box's row, and what goes above the
 *  row: the one being sent, with how far it is, then those sent, each with
 *  its path to copy. Nothing is typed into the pane. */
export function useUploads(pane: string): { button: ReactNode; panel: ReactNode } {
  const [place, setPlace] = useState<Place | null>(null);
  const [going, setGoing] = useState<Going | null>(null);
  const [sent, setSent] = useState<Sent[]>([]);
  const picker = useRef<HTMLInputElement>(null);

  // Asked ahead: the picker opens on the tap itself (a phone opens it only
  // then), so what to say is known by then.
  useEffect(() => {
    let live = true;
    setPlace(null);
    getJson<{ dir: string; max: number }>(`/api/upload-info?pane=${q(pane)}`)
      .then((p) => live && setPlace(p))
      .catch((e: Error) => live && setPlace({ error: e.message }));
    return () => {
      live = false;
    };
  }, [pane]);

  const pick = () => {
    if (place && "error" in place) return void toast.danger(place.error);
    picker.current?.click();
  };

  const chosen = async (files: File[]) => {
    const max = place && "max" in place ? place.max : Infinity;
    for (const [i, file] of files.entries()) {
      if (file.size > max) {
        toast.danger(t(`${file.name}: ${size(file.size)} is more than ${size(max)} (web-upload-max)`, `${file.name}：${size(file.size)}，超过上限 ${size(max)}（web-upload-max）`));
        continue;
      }
      setGoing({ name: file.name, done: 0, total: file.size, nth: i + 1, of: files.length });
      try {
        const s = await sendOne(pane, file, (done, total) => setGoing((g) => g && { ...g, done, total }));
        setSent((now) => [s, ...now].slice(0, 5));
      } catch (e) {
        toast.danger(`${file.name}: ${(e as Error).message}`);
      }
    }
    setGoing(null);
  };

  const dir = place && "dir" in place ? place.dir : "";
  const button = (
    <>
      <Button id="upload" isIconOnly variant="ghost" size="sm" className="mb-0.5 pointer-coarse:size-11" isDisabled={!!going}
        aria-label={dir ? t(`Send a file into ${dir}`, `上传文件到 ${dir}`) : t("Send a file", "上传文件")} onPress={pick}>
        <Paperclip className="size-4.5" />
      </Button>
      <input ref={picker} type="file" multiple className="hidden" data-testid="upload-input"
        onChange={(e) => {
          const files = Array.from(e.currentTarget.files ?? []);
          e.currentTarget.value = "";
          if (files.length) void chosen(files);
        }} />
    </>
  );

  const panel =
    going || sent.length ? (
      <div id="uploads" className="mb-1.5 rounded-lg border border-separator bg-surface px-2 py-1.5 text-xs">
        {going ? (
          <div className="pb-1">
            <div className="flex justify-between gap-2">
              <span className="truncate">
                {going.of > 1 ? `${going.nth}/${going.of} · ` : ""}
                {t(`Sending ${going.name}`, `正在上传 ${going.name}`)}
              </span>
              <span className="shrink-0 text-muted">{going.total ? Math.floor((going.done / going.total) * 100) : 0}%</span>
            </div>
            <div className="mt-1 h-1 overflow-hidden rounded bg-default">
              <div className="h-full bg-accent transition-[width]" style={{ width: `${going.total ? (going.done / going.total) * 100 : 0}%` }} />
            </div>
          </div>
        ) : null}
        {sent.map((s) => (
          <div key={s.path} className="flex items-center gap-1">
            <span className="min-w-0 flex-1 truncate" title={s.path}>
              {t("Saved", "已存到")} <span className="font-term">{s.path}</span>
            </span>
            <Button isIconOnly size="sm" variant="ghost" className="size-8 min-w-8 pointer-coarse:size-11" aria-label={t(`Copy the path of ${s.name}`, `复制 ${s.name} 的路径`)}
              onPress={async () => (await copyText(s.path)) && toast.success(t("Path copied", "路径已复制"), { timeout: 1500 })}>
              <Copy className="size-3.5" />
            </Button>
          </div>
        ))}
        {sent.length && !going ? (
          <div className="flex justify-end">
            <Button size="sm" variant="ghost" className="h-7 px-1.5 text-xs pointer-coarse:h-11 pointer-coarse:min-w-11" onPress={() => setSent([])} aria-label={t("Clear the list", "清空列表")}>
              <X className="size-3" /> {t("Clear", "清空")}
            </Button>
          </div>
        ) : null}
      </div>
    ) : null;

  return { button, panel };
}
