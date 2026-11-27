import { AlertDialog, Button, Chip, Dropdown, Label, toast } from "@heroui/react";
import { Clock, Copy, Eye, FileText, Folder, Link2, Square, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { getJson, post } from "../api";
import { copyText } from "../copy";
import { useVisible } from "../hooks";
import { t } from "../i18n";
import { Preview } from "./Files";

// What this machine shares (`keepane share`, docs/design/share.md), as the
// page shows it: each share's state, time left, downloads and link; a file
// shown, a share stopped or given more time. The terminal's board has the
// same on its shares page.

/** A share, as `share -l -J` gives it. */
type Share = {
  id: number;
  state: "active" | "missing" | "stopped" | "expired" | "used";
  kind: "file" | "folder";
  name: string;
  path: string;
  /** When it ends (Unix milliseconds); null: no end. */
  until: number | null;
  once: boolean;
  /** Times all of it went. */
  sent: number;
  links: string[];
  ended: number | null;
  /** What keepane shared it for: `group file #chan #185`. */
  origin?: string | null;
};

const LIVE = new Set(["active", "missing"]);

/** Time left, as people read it: `45m left`, `3h left`, `2d left`. */
function left(until: number | null): string {
  if (until === null) return t("no end", "不限时");
  const m = Math.max(0, Math.round((until - Date.now()) / 60_000));
  const said = m < 60 ? `${m}m` : m < 48 * 60 ? `${Math.round(m / 60)}h` : `${Math.round(m / 1440)}d`;
  return t(`${said} left`, `剩 ${said}`);
}

const STATE: Record<Share["state"], { en: string; zh: string; color: "success" | "warning" | "default" }> = {
  active: { en: "active", zh: "分享中", color: "success" },
  missing: { en: "file missing", zh: "文件不在", color: "warning" },
  stopped: { en: "stopped", zh: "已停止", color: "default" },
  expired: { en: "expired", zh: "已到期", color: "default" },
  used: { en: "used", zh: "已用过", color: "default" },
};

export function SharesView({ wide, readOnly, onClose }: { wide: boolean; readOnly: boolean; onClose: () => void }) {
  const visible = useVisible();
  const [shares, setShares] = useState<Share[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ended, setEnded] = useState(false);
  const [open, setOpen] = useState<Share | null>(null);
  const [stopping, setStopping] = useState<Share | null>(null);
  const load = useCallback(async () => {
    try {
      setShares((await getJson<{ shares: Share[] }>("/api/shares")).shares);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  }, []);
  // Read every three seconds while shown: downloads and time left move.
  useEffect(() => {
    if (!visible) return;
    let stop = false;
    let timer: number | undefined;
    const loop = async () => {
      await load();
      if (!stop) timer = window.setTimeout(loop, 3000);
    };
    loop();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [visible, load]);
  const act = async (path: string, done: string) => {
    try {
      await post(path);
      toast.success(done);
    } catch (e) {
      toast.danger((e as Error).message);
    }
    load();
  };
  const shown = (shares || []).filter((s) => ended || LIVE.has(s.state));
  const endedCount = (shares || []).length - (shares || []).filter((s) => LIVE.has(s.state)).length;
  return (
    <section className="thin-scroll flex min-h-0 flex-1 flex-col overflow-y-auto" aria-label={t("Shares", "分享")}>
      <div className={"flex w-full items-center gap-2 px-4 pt-3 pb-2 " + (wide ? "mx-auto max-w-3xl" : "")}>
        <Link2 className="size-4.5 text-muted" />
        <h2 className="flex-1 text-[15px] font-semibold">{t("Shares", "分享")}</h2>
        {endedCount > 0 ? (
          <Button variant={ended ? "secondary" : "ghost"} size="sm" aria-pressed={ended} onPress={() => setEnded(!ended)}>
            {t(`Ended (${endedCount})`, `已结束（${endedCount}）`)}
          </Button>
        ) : null}
        <Button isIconOnly variant="ghost" size="sm" aria-label={t("Back to the panes", "回到 pane 列表")} onPress={onClose}>
          <X className="size-4.5" />
        </Button>
      </div>
      <div className={"w-full px-2 pb-4 " + (wide ? "mx-auto max-w-3xl" : "")}>
        {error ? <p className="mx-2 rounded-xl bg-danger-soft p-3 text-sm text-danger-soft-foreground">{error}</p> : null}
        {shares && shown.length === 0 ? (
          <p className="px-2 py-6 text-sm text-muted">{t("Nothing shared now (keepane share PATH).", "现在没有分享（keepane share 路径）。")}</p>
        ) : null}
        <ul className="flex flex-col gap-1">
          {shown.map((s) => {
            const live = LIVE.has(s.state);
            const st = STATE[s.state] || { en: s.state, zh: s.state, color: "default" as const };
            return (
              <li key={s.id} className={"rounded-2xl border border-separator px-3 py-2.5 " + (live ? "" : "opacity-60")} aria-label={`#${s.id} ${s.name}`}>
                <div className="flex items-center gap-2">
                  {s.kind === "folder" ? <Folder className="size-4 shrink-0 text-muted" aria-hidden /> : <FileText className="size-4 shrink-0 text-muted" aria-hidden />}
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">{s.name}</span>
                  <Chip size="sm" variant="soft" color={st.color}>
                    {t(st.en, st.zh)}
                  </Chip>
                </div>
                <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted">
                  <span>#{s.id}</span>
                  {live ? <span>{left(s.until)}</span> : null}
                  {s.once ? <span>{t("once", "一次性")}</span> : null}
                  <span>{t(`downloaded ${s.sent}×`, `下载 ${s.sent} 次`)}</span>
                  {s.origin ? <span>{t(`for ${s.origin}`, `用于 ${s.origin}`)}</span> : null}
                </div>
                {live && s.links[0] ? <div className="mt-1 truncate font-mono text-xs text-muted select-text">{s.links[0]}</div> : null}
                <div className="mt-1.5 flex flex-wrap gap-1">
                  {s.kind === "file" ? (
                    <Button size="sm" variant="ghost" aria-label={t(`Show ${s.name}`, `查看 ${s.name}`)} onPress={() => setOpen(s)}>
                      <Eye className="size-4" />
                      {t("Show", "查看")}
                    </Button>
                  ) : null}
                  {live && s.links[0] ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-label={t(`Copy the link of #${s.id}`, `复制 #${s.id} 的链接`)}
                      onPress={async () => ((await copyText(s.links[0])) ? toast.success(t("Link copied", "链接已复制")) : toast.danger(t("Could not copy", "复制失败")))}
                    >
                      <Copy className="size-4" />
                      {t("Copy link", "复制链接")}
                    </Button>
                  ) : null}
                  {!readOnly && (live || s.state === "expired") ? (
                    <Dropdown>
                      <Button size="sm" variant="ghost" aria-label={t(`More time for #${s.id}`, `延长 #${s.id}`)}>
                        <Clock className="size-4" />
                        {t("Extend", "延长")}
                      </Button>
                      <Dropdown.Popover>
                        <Dropdown.Menu aria-label={t("How long", "延长多久")} onAction={(k) => act(`/api/share/extend?id=${s.id}&for=${String(k)}`, t(`#${s.id} extended`, `#${s.id} 已延长`))}>
                          {(["1h", "24h", "7d", "0"] as const).map((d) => (
                            <Dropdown.Item key={d} id={d} textValue={d}>
                              {/* `share extend` ends it this long from now (not added to what is left). */}
                              <Label>{d === "0" ? t("No end", "不限时") : t(`Ends ${d} from now`, `从现在起 ${d}`)}</Label>
                            </Dropdown.Item>
                          ))}
                        </Dropdown.Menu>
                      </Dropdown.Popover>
                    </Dropdown>
                  ) : null}
                  {!readOnly && live ? (
                    <Button size="sm" variant="ghost" className="text-danger" aria-label={t(`Stop #${s.id}`, `停止 #${s.id}`)} onPress={() => setStopping(s)}>
                      <Square className="size-4" />
                      {t("Stop", "停止")}
                    </Button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      </div>
      <Preview name={open?.name || ""} view={open ? `/api/share/file?id=${open.id}` : null} onClose={() => setOpen(null)} />
      <AlertDialog.Backdrop isOpen={!!stopping} onOpenChange={(o) => !o && setStopping(null)} isKeyboardDismissDisabled={false}>
        <AlertDialog.Container size="sm">
          <AlertDialog.Dialog>
            <AlertDialog.Header>
              <AlertDialog.Icon status="danger" />
              <AlertDialog.Heading>{t(`Stop sharing ${stopping?.name}?`, `停止分享 ${stopping?.name}？`)}</AlertDialog.Heading>
            </AlertDialog.Header>
            <AlertDialog.Body>
              <p>{t("Its link stops working at once.", "它的链接立刻失效。")}</p>
            </AlertDialog.Body>
            <AlertDialog.Footer>
              <Button variant="secondary" onPress={() => setStopping(null)}>
                {t("Cancel", "取消")}
              </Button>
              <Button
                variant="danger"
                onPress={() => {
                  const s = stopping;
                  setStopping(null);
                  if (s) act(`/api/share/stop?id=${s.id}`, t(`#${s.id} stopped`, `#${s.id} 已停止`));
                }}
              >
                {t("Stop", "停止")}
              </Button>
            </AlertDialog.Footer>
          </AlertDialog.Dialog>
        </AlertDialog.Container>
      </AlertDialog.Backdrop>
    </section>
  );
}
