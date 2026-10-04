import { AlertDialog, Button, Drawer, Input, Modal, toast } from "@heroui/react";
import { Check, Star, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { getJson, post, q, type Inbox, type Pane } from "../api";
import { bounced, headOf, quietFor, where } from "../format";
import { useMedia } from "../hooks";
import { t } from "../i18n";

/** Focus brought into an overlay that has just opened. One opened from a
 *  menu loses it otherwise: the menu, closing, gives focus back to its
 *  button, which the overlay has made inert, and it lands nowhere (on the
 *  body), so Escape and Tab do nothing. */
export function useFocusInside<T extends HTMLElement = HTMLDivElement>(open: boolean) {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (!open) return;
    const tick = window.setInterval(() => {
      const dialog = ref.current?.closest('[role="dialog"], [role="alertdialog"]') as HTMLElement | null;
      if (dialog && !dialog.contains(document.activeElement)) dialog.focus();
    }, 100);
    const stop = window.setTimeout(() => clearInterval(tick), 600);
    return () => {
      clearInterval(tick);
      clearTimeout(stop);
    };
  }, [open]);
  return ref;
}

/** A sheet: from the bottom on a phone, from the right on a wide screen. */
export function Sheet({
  open,
  onClose,
  title,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
}) {
  const wide = useMedia("(min-width: 960px)");
  const inside = useFocusInside(open);
  return (
    <Drawer.Backdrop
      isOpen={open}
      onOpenChange={(o) => !o && onClose()}
      variant="blur"
      isDismissable
      isKeyboardDismissDisabled={false}
    >
      <Drawer.Content placement={wide ? "right" : "bottom"}>
        <Drawer.Dialog className={wide ? "w-[420px] max-w-full" : "max-h-[80dvh]"}>
          {wide ? null : <Drawer.Handle />}
          <Drawer.Header>
            <Drawer.Heading>{title}</Drawer.Heading>
          </Drawer.Header>
          <Drawer.Body className="thin-scroll">
            <div ref={inside}>{children}</div>
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

/** Every pane, to go to. */
export function Switcher({
  open,
  onClose,
  panes,
  current,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  panes: Pane[];
  current: string | null;
  onPick: (id: string) => void;
}) {
  const rows: ReactNode[] = [];
  let lastS: string | null = null;
  let lastW: number | null = null;
  for (const p of panes) {
    if (p.session !== lastS) {
      rows.push(
        <div key={"s" + p.session} className="mt-3 text-xs font-semibold tracking-wide text-muted first:mt-0">
          {p.session}
        </div>,
      );
      lastS = p.session;
      lastW = null;
    }
    if (p.window !== lastW) {
      rows.push(
        <div key={"w" + p.session + p.window} className="mt-1 pl-2 text-xs text-muted">
          {p.window}: {p.windowName}
        </div>,
      );
      lastW = p.window;
    }
    rows.push(
      <Button
        key={p.id}
        fullWidth
        variant={p.id === current ? "secondary" : "ghost"}
        className="justify-start"
        onPress={() => {
          onClose();
          onPick(p.id);
        }}
      >
        <span className="min-w-0 flex-1 truncate text-left">
          {headOf(p)}
          {p.last ? <span className="text-muted"> — {p.last}</span> : null}
        </span>
        <span className="shrink-0 text-xs text-muted">{where(p)}</span>
      </Button>,
    );
  }
  return (
    <Sheet open={open} onClose={onClose} title={t("Go to a pane", "切换到 pane")}>
      <div className="flex flex-col gap-0.5 pb-2">{rows}</div>
    </Sheet>
  );
}

export type Sent = { t: string; star: boolean };

/** What was sent from here, newest first; starred ones stay at the top. */
export function History({
  open,
  onClose,
  sent,
  setSent,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  sent: Sent[];
  setSent: (s: Sent[]) => void;
  onPick: (text: string) => void;
}) {
  const row = (s: Sent, i: number) => (
    <div key={i} className="flex items-center gap-1">
      <Button
        fullWidth
        variant="ghost"
        className="justify-start font-term text-sm"
        onPress={() => {
          onClose();
          onPick(s.t);
        }}
      >
        <span className="truncate">{s.t}</span>
      </Button>
      <Button
        isIconOnly
        size="sm"
        variant="ghost"
        aria-label={t("Star", "常用")}
        className={s.star ? "text-warning" : "text-muted"}
        onPress={() => setSent(sent.map((x, j) => (j === i ? { ...x, star: !x.star } : x)))}
      >
        <Star className="size-4" fill={s.star ? "currentColor" : "none"} />
      </Button>
      <Button
        isIconOnly
        size="sm"
        variant="ghost"
        aria-label={t("Remove", "删除")}
        className="text-muted"
        onPress={() => setSent(sent.filter((_, j) => j !== i))}
      >
        <X className="size-4" />
      </Button>
    </div>
  );
  const starred = sent.map((s, i) => [s, i] as const).filter(([s]) => s.star);
  const recent = sent.map((s, i) => [s, i] as const).filter(([s]) => !s.star);
  return (
    <Sheet open={open} onClose={onClose} title={t("Sent before", "发过的命令")}>
      <p className="mb-2 text-xs text-muted">
        {t("Tap one to put it in the box; ☆ keeps it at the top.", "点一条放进输入框；☆ 设为常用，固定在最上面。")}
      </p>
      {starred.length ? <div className="mt-2 mb-1 text-xs font-semibold text-muted">{t("Starred", "常用")}</div> : null}
      {starred.map(([s, i]) => row(s, i))}
      {recent.length ? <div className="mt-2 mb-1 text-xs font-semibold text-muted">{t("Recent", "最近")}</div> : null}
      {recent.map(([s, i]) => row(s, i))}
      {!sent.length ? <p className="py-6 text-center text-sm text-muted">{t("Nothing sent yet.", "还没有发过命令。")}</p> : null}
    </Sheet>
  );
}

/** A pane's inbox: what it works on, what waits (to the top, up, down, or
 *  deleted, and the last deleted brought back), what it finished lately.
 *  Asked for again every three seconds while it is open. */
export function InboxSheet({
  pane,
  onClose,
  panes,
  readOnly,
}: {
  pane: string | null;
  onClose: () => void;
  panes: Pane[];
  readOnly: boolean;
}) {
  const [d, setD] = useState<Inbox>(null);
  const [err, setErr] = useState<string | null>(null);
  const [dropped, setDropped] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const load = useCallback(async () => {
    clearTimeout(timer.current);
    if (!pane) return;
    try {
      setD(await getJson<Inbox>(`/api/inbox?pane=${q(pane)}`));
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    }
    if (document.visibilityState === "visible") timer.current = window.setTimeout(load, 3000);
  }, [pane]);
  useEffect(() => {
    setD(null);
    setDropped(null);
    load();
    return () => clearTimeout(timer.current);
  }, [load]);
  const act = async (what: string, id?: number) => {
    if (bounced("msg " + what + (id ?? ""))) return;
    try {
      await post(`/api/message?do=${what}${id ? `&id=${id}` : ""}`);
      setDropped(what === "drop" ? String(id) : null);
    } catch (e) {
      toast.danger((e as Error).message);
    }
    load();
  };
  const p = panes.find((x) => x.id === pane);
  const queued = d?.queued || [];
  const recent = d?.recent || [];
  const Msg = ({ m, meta, acts }: { m: { id: number; from: string; text: string }; meta?: string; acts?: ReactNode }) => (
    <div className="rounded-xl border border-border bg-surface p-3">
      <div className="text-xs text-muted">
        #{m.id} · {m.from}
        {meta}
      </div>
      <pre className="mt-1.5 max-h-32 overflow-auto font-term text-[13px] whitespace-pre-wrap [overflow-wrap:anywhere]">{m.text}</pre>
      {acts}
    </div>
  );
  return (
    <Sheet open={!!pane} onClose={onClose} title={t(`${p ? headOf(p) : pane}: inbox`, `${p ? headOf(p) : pane} 的收件箱`)}>
      {err ? <p className="text-sm text-danger">{err}</p> : null}
      {dropped && !readOnly ? (
        <div className="mb-2 flex items-center gap-2 rounded-xl bg-default px-3 py-2 text-sm">
          <span className="flex-1">{t(`Deleted #${dropped}`, `已删除 #${dropped}`)}</span>
          <Button size="sm" variant="secondary" onPress={() => act("undo")}>
            {t("Undo", "撤销")}
          </Button>
        </div>
      ) : null}
      <Section title={t("Working on", "正在处理")}>
        {d?.current ? <Msg m={d.current} /> : <None>{t("Nothing", "没有")}</None>}
      </Section>
      <Section title={t(`Waiting (${queued.length})`, `排队（${queued.length} 条）`)}>
        {!queued.length ? <None>{t("Nothing waits", "没有排队的消息")}</None> : null}
        {queued.map((m, i) => (
          <Msg
            key={m.id}
            m={m}
            meta={
              ` · ${t("waiting", "等了")} ${quietFor(m.waited)}` + (m.for ? ` · ${t(`sent for ${m.for}`, `发给 ${m.for} 模式的`)}` : "")
            }
            acts={
              readOnly ? null : (
                <div className="mt-2 flex gap-1.5">
                  <Button size="sm" variant="secondary" isDisabled={i === 0} onPress={() => act("top", m.id)}>
                    {t("Top", "置顶")}
                  </Button>
                  <Button size="sm" variant="secondary" isDisabled={i === 0} onPress={() => act("up", m.id)}>
                    ↑
                  </Button>
                  <Button size="sm" variant="secondary" isDisabled={i === queued.length - 1} onPress={() => act("down", m.id)}>
                    ↓
                  </Button>
                  <Button size="sm" variant="danger" className="ml-auto" onPress={() => act("drop", m.id)}>
                    <Trash2 className="size-3.5" />
                    {t("Delete", "删除")}
                  </Button>
                </div>
              )
            }
          />
        ))}
      </Section>
      {recent.length ? (
        <Section title={t("Finished lately", "最近完成")}>
          {recent.map((m) => {
            const bad = m.ok === false || m.stage !== "done";
            return (
              <div key={m.id} className="flex items-center gap-2 text-sm">
                {bad ? <X className="size-3.5 text-danger" /> : <Check className="size-3.5 text-success" />}
                <span className="text-muted">
                  #{m.id} {m.stage}
                </span>
                <span className="truncate">{(m.text || "").split("\n")[0]}</span>
              </div>
            );
          })}
        </Section>
      ) : null}
    </Sheet>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="mb-4">
      <div className="mb-1.5 text-xs font-semibold tracking-wide text-muted">{title}</div>
      <div className="flex flex-col gap-2">{children}</div>
    </div>
  );
}
const None = ({ children }: { children: ReactNode }) => <p className="text-sm text-muted">{children}</p>;

/** Asking for a name: resolves the dialog with it, or nothing. */
export function RenameDialog({
  ask,
  onDone,
}: {
  ask: { title: string; value: string } | null;
  onDone: (name: string | null) => void;
}) {
  const [v, setV] = useState("");
  useEffect(() => setV(ask?.value || ""), [ask]);
  const inside = useFocusInside<HTMLFormElement>(!!ask);
  return (
    <Modal.Backdrop isOpen={!!ask} onOpenChange={(o) => !o && onDone(null)} isKeyboardDismissDisabled={false}>
      <Modal.Container size="sm">
        <Modal.Dialog>
          <Modal.Header>
            <Modal.Heading>{ask?.title}</Modal.Heading>
          </Modal.Header>
          <Modal.Body>
            <form
              ref={inside}
              onSubmit={(e) => {
                e.preventDefault();
                onDone(v.trim());
              }}
            >
              <Input
                autoFocus
                fullWidth
                value={v}
                onChange={(e) => setV(e.target.value)}
                autoCapitalize="off"
                autoComplete="off"
                autoCorrect="off"
                spellCheck={false}
                enterKeyHint="done"
              />
            </form>
          </Modal.Body>
          <Modal.Footer>
            <Button variant="secondary" onPress={() => onDone(null)}>
              {t("Cancel", "取消")}
            </Button>
            <Button onPress={() => onDone(v.trim())}>{t("OK", "确定")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

/** Closing a pane: what runs in it stops, so it is asked first. */
/** Closing a pane: says which, and what runs in it. */
export function ConfirmClose({
  open,
  onDone,
  what,
  program,
}: {
  open: boolean;
  onDone: (yes: boolean) => void;
  what?: string;
  program?: string;
}) {
  const inside = useFocusInside<HTMLParagraphElement>(open);
  return (
    <AlertDialog.Backdrop isOpen={open} onOpenChange={(o) => !o && onDone(false)} isKeyboardDismissDisabled={false}>
      <AlertDialog.Container size="sm">
        <AlertDialog.Dialog>
          <AlertDialog.Header>
            <AlertDialog.Icon status="danger" />
            <AlertDialog.Heading>
              {what ? t(`Close ${what}?`, `关闭 ${what}？`) : t("Close this pane?", "关闭这个 pane？")}
            </AlertDialog.Heading>
          </AlertDialog.Header>
          <AlertDialog.Body>
            <p ref={inside}>
              {program
                ? t(`${program} in it stops.`, `里面运行的 ${program} 会被结束。`)
                : t("What runs in it stops.", "里面运行的程序会被结束。")}
            </p>
          </AlertDialog.Body>
          <AlertDialog.Footer>
            <Button variant="secondary" onPress={() => onDone(false)}>
              {t("Cancel", "取消")}
            </Button>
            <Button variant="danger" onPress={() => onDone(true)}>
              {t("Close", "关闭")}
            </Button>
          </AlertDialog.Footer>
        </AlertDialog.Dialog>
      </AlertDialog.Container>
    </AlertDialog.Backdrop>
  );
}
