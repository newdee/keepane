import { AlertDialog, Button, Modal, toast } from "@heroui/react";
import { Download, ExternalLink, FileText, Image as Picture, Paperclip, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, getJson, post, q, saveFile } from "../api";
import { t } from "../i18n";
import { useFocusInside } from "./Sheets";

// A file shown on the page: a group file of a channel (docs/design/chat.md
// §13) or a file this machine shares. The server says what it is by its
// first bytes (`view=1`); a picture and a PDF are shown from a blob, text as
// text, so an SVG or a web page in a file is read, never drawn or run.

/** A size as people read it: `512 B`, `1.5 KB`, `3.2 MB`. */
export const sizeText = (b: number) =>
  b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(1)} MB` : b >= 1024 ? `${(b / 1024).toFixed(1)} KB` : `${b} B`;

/** What a preview shows, once asked. */
type Shown =
  | { kind: "loading" }
  | { kind: "picture" | "pdf"; url: string }
  | { kind: "text"; text: string; cut: number | null }
  | { kind: "none"; why: string };

/** The file at `path` (an address that shows a file: `/api/chat/file?…&view=1`,
 *  `/api/share/file?id=N`), fetched with the key. */
function useShown(path: string | null): Shown {
  const [shown, setShown] = useState<Shown>({ kind: "loading" });
  useEffect(() => {
    if (!path) return;
    let url = "";
    let stop = false;
    setShown({ kind: "loading" });
    (async () => {
      try {
        const r = await api(path);
        const type = r.headers.get("Content-Type") || "";
        if (type.startsWith("text/plain")) {
          const cut = r.headers.get("X-Keepane-Cut");
          const text = await r.text();
          if (!stop) setShown({ kind: "text", text, cut: cut ? Number(cut) : null });
          return;
        }
        const picture = type.startsWith("image/");
        if (!picture && type !== "application/pdf") throw new Error(t("No preview for this file.", "这个文件不能预览。"));
        const blob = await r.blob();
        // Closed meanwhile: nothing to keep.
        if (stop) return;
        url = URL.createObjectURL(blob);
        setShown({ kind: picture ? "picture" : "pdf", url });
      } catch (e) {
        const why = (e as Error).message;
        // The server's 415: said in the page's language.
        const said = why.includes("larger than")
          ? t("Too big to show here: save it to see it.", "文件太大，不在这里显示：保存后再看。")
          : why.startsWith("no preview")
            ? t("No preview for this file.", "这个文件不能预览。")
            : why;
        if (!stop) setShown({ kind: "none", why: said });
      }
    })();
    return () => {
      stop = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [path]);
  return shown;
}

/** A file shown over the page: `view` is where it is shown from, `save`
 *  where it is saved from (none: not saved here). */
export function Preview({
  name,
  view,
  save,
  onClose,
}: {
  name: string;
  view: string | null;
  save?: string;
  onClose: () => void;
}) {
  const shown = useShown(view);
  const inside = useFocusInside(!!view);
  return (
    <Modal.Backdrop isOpen={!!view} onOpenChange={(o) => !o && onClose()} isKeyboardDismissDisabled={false}>
      <Modal.Container size="lg">
        <Modal.Dialog aria-label={t(`Preview of ${name}`, `${name} 的预览`)}>
          <Modal.Header>
            <Modal.Heading className="truncate">{name}</Modal.Heading>
          </Modal.Header>
          <Modal.Body>
            <div ref={inside} className="min-h-24">
              {shown.kind === "loading" ? <p className="py-8 text-center text-sm text-muted">{t("Loading…", "加载中…")}</p> : null}
              {shown.kind === "none" ? <p className="rounded-xl bg-surface p-3 text-sm text-muted">{shown.why}</p> : null}
              {shown.kind === "picture" ? <img src={shown.url} alt={name} className="mx-auto max-h-[70dvh] max-w-full object-contain" /> : null}
              {shown.kind === "pdf" ? <iframe src={shown.url} title={name} className="h-[70dvh] w-full rounded-xl border border-separator" /> : null}
              {shown.kind === "text" ? (
                <>
                  {shown.cut !== null ? (
                    <p className="mb-2 text-xs text-muted">
                      {t(`The first ${sizeText(shown.text.length)} of ${sizeText(shown.cut)}.`, `只显示前 ${sizeText(shown.text.length)}（共 ${sizeText(shown.cut)}）。`)}
                    </p>
                  ) : null}
                  <pre aria-label={t("Its text", "文件内容")} className="thin-scroll max-h-[70dvh] overflow-auto rounded-xl bg-surface p-3 font-mono text-xs whitespace-pre-wrap break-words select-text">
                    {shown.text || t("(empty)", "（空文件）")}
                  </pre>
                </>
              ) : null}
            </div>
          </Modal.Body>
          <Modal.Footer>
            {shown.kind === "pdf" ? (
              <Button variant="secondary" onPress={() => window.open(shown.url, "_blank", "noopener")}>
                <ExternalLink className="size-4" />
                {t("Open", "打开")}
              </Button>
            ) : null}
            {save ? (
              <Button
                variant="secondary"
                onPress={() => saveFile(save, name).catch((e: Error) => toast.danger(e.message))}
              >
                <Download className="size-4" />
                {t("Save", "保存")}
              </Button>
            ) : null}
            <Button onPress={onClose}>{t("Close", "关闭")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

/** A group file, as `chat-group-files -J` gives it. */
export type GroupFile = {
  file: string;
  name: string;
  size: number;
  from: string;
  seq: number;
  at: string;
  missing?: string;
};

const isPicture = (name: string) => /\.(png|jpe?g|gif|webp)$/i.test(name);

/** Beside a channel: its group files, the newest first; one opened shows
 *  over the page, saved from there or its row, deleted from its row. `last` is the
 *  channel's last line: a new one may have brought a file. */
export function GroupFiles({
  channel,
  last,
  wide,
  readOnly,
  onClose,
}: {
  channel: string;
  last: number;
  wide: boolean;
  readOnly: boolean;
  onClose: () => void;
}) {
  const [files, setFiles] = useState<GroupFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<GroupFile | null>(null);
  const [dropping, setDropping] = useState<GroupFile | null>(null);
  const load = useCallback(async () => {
    try {
      setFiles((await getJson<{ group_files: GroupFile[] }>(`/api/chat/group-files?c=${q(channel)}`)).group_files);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  }, [channel]);
  // Read when a line comes (it may bring a file), and every five seconds
  // while shown (one deleted elsewhere goes).
  useEffect(() => {
    load();
    const timer = window.setInterval(load, 5000);
    return () => clearInterval(timer);
  }, [load, last]);
  const at = (f: GroupFile) => `/api/chat/file?c=${q(channel)}&f=${q(f.file)}`;
  const drop = async (f: GroupFile) => {
    setDropping(null);
    try {
      await post(`/api/chat/group-file-rm?c=${q(channel)}&f=${q(f.file)}`);
      toast.success(t(`Deleted ${f.name}`, `已删除 ${f.name}`));
      if (open?.file === f.file) setOpen(null);
    } catch (e) {
      toast.danger((e as Error).message);
    }
    load();
  };
  return (
    <aside
      className={
        "thin-scroll min-h-0 overflow-y-auto border-l border-separator bg-background " +
        (wide ? "w-[300px] shrink-0" : "absolute inset-x-0 bottom-0 top-auto z-20 max-h-[60%] rounded-t-3xl border-t shadow-2xl")
      }
      aria-label={t("Group files", "群文件")}
    >
      <div className="flex items-center gap-2 px-4 pt-3 pb-1">
        <h3 className="flex-1 text-sm font-semibold">{t("Group files", "群文件")}</h3>
        <Button isIconOnly variant="ghost" size="sm" aria-label={t("Close", "关闭")} onPress={onClose}>
          <X className="size-4" />
        </Button>
      </div>
      {error ? <p className="mx-4 rounded-xl bg-danger-soft p-3 text-sm text-danger-soft-foreground">{error}</p> : null}
      {files && files.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted">{t("None yet: a file sent here is kept until deleted.", "还没有：在这里发的文件会一直保留，直到删除。")}</p>
      ) : null}
      <ul className="flex flex-col gap-0.5 px-2 pb-3">
        {(files || []).map((f) => (
          <li key={f.file} className={"flex items-center gap-1 rounded-xl pr-1 hover:bg-surface " + (f.missing ? "opacity-55" : "")}>
            <button
              type="button"
              className="flex min-w-0 flex-1 items-center gap-3 rounded-xl px-2 py-2 text-left pointer-coarse:min-h-11"
              aria-label={t(`Show ${f.name}`, `查看 ${f.name}`)}
              disabled={!!f.missing}
              onClick={() => setOpen(f)}
            >
              {isPicture(f.name) ? <Picture className="size-4 shrink-0 text-muted" aria-hidden /> : <FileText className="size-4 shrink-0 text-muted" aria-hidden />}
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm">{f.name}</span>
                <span className="block truncate text-xs text-muted">
                  {f.missing ? f.missing : [sizeText(f.size), f.from, `#${f.seq}`, f.at.slice(0, 10)].filter(Boolean).join(" · ")}
                </span>
              </span>
            </button>
            {!f.missing ? (
              <Button isIconOnly variant="ghost" size="sm" className="pointer-coarse:size-11" aria-label={t(`Save ${f.name}`, `保存 ${f.name}`)} onPress={() => saveFile(at(f), f.name).catch((e: Error) => toast.danger(e.message))}>
                <Download className="size-4" />
              </Button>
            ) : null}
            {!readOnly ? (
              <Button isIconOnly variant="ghost" size="sm" className="pointer-coarse:size-11" aria-label={t(`Delete ${f.name}`, `删除 ${f.name}`)} onPress={() => setDropping(f)}>
                <Trash2 className="size-4" />
              </Button>
            ) : null}
          </li>
        ))}
      </ul>
      <Preview name={open?.name || ""} view={open ? `${at(open)}&view=1` : null} save={open ? at(open) : undefined} onClose={() => setOpen(null)} />
      <AlertDialog.Backdrop isOpen={!!dropping} onOpenChange={(o) => !o && setDropping(null)} isKeyboardDismissDisabled={false}>
        <AlertDialog.Container size="sm">
          <AlertDialog.Dialog>
            <AlertDialog.Header>
              <AlertDialog.Icon status="danger" />
              <AlertDialog.Heading>{t(`Delete ${dropping?.name}?`, `删除 ${dropping?.name}？`)}</AlertDialog.Heading>
            </AlertDialog.Header>
            <AlertDialog.Body>
              <p>{t("Its copy goes, and the link another machine was given for it.", "它的副本会删掉，给别的机器的链接也会失效。")}</p>
            </AlertDialog.Body>
            <AlertDialog.Footer>
              <Button variant="secondary" onPress={() => setDropping(null)}>
                {t("Cancel", "取消")}
              </Button>
              <Button variant="danger" onPress={() => dropping && drop(dropping)}>
                {t("Delete", "删除")}
              </Button>
            </AlertDialog.Footer>
          </AlertDialog.Dialog>
        </AlertDialog.Container>
      </AlertDialog.Backdrop>
    </aside>
  );
}

/** A line's file: a chip that shows it (saved from there). */
export function FileChip({ name, size, channel, file }: { name: string; size: number; channel: string; file: string }) {
  const [open, setOpen] = useState(false);
  const at = `/api/chat/file?c=${q(channel)}&f=${q(file)}`;
  return (
    <>
      <button
        type="button"
        className="flex items-center gap-2 rounded-xl border border-separator bg-background/60 px-2.5 py-1.5 text-left text-[13px] text-foreground hover:bg-background pointer-coarse:min-h-11"
        aria-label={t(`Show ${name}`, `查看 ${name}`)}
        onClick={() => setOpen(true)}
      >
        <Paperclip className="size-4 shrink-0 text-muted" aria-hidden />
        <span className="min-w-0 flex-1 truncate">{name}</span>
        <span className="shrink-0 text-xs text-muted">{sizeText(size)}</span>
      </button>
      <Preview name={name} view={open ? `${at}&view=1` : null} save={at} onClose={() => setOpen(false)} />
    </>
  );
}
