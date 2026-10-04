import { Button, Dropdown, Header, Label, Separator } from "@heroui/react";
import { Check } from "lucide-react";
import { Fragment, useRef, type ReactNode } from "react";

export type MenuEntry = {
  id: string;
  label: string;
  icon: ReactNode;
  danger?: boolean;
  /** A tick: the one chosen of a few. */
  checked?: boolean;
  /** A new group starts here, under this heading. */
  section?: string;
  /** Runs inside the tap itself (copying to the clipboard must). */
  now?: boolean;
};

/** A menu opened at a point: where the right click or the long press was. */
export type MenuAt = {
  x: number;
  y: number;
  title: string;
  items: MenuEntry[];
  onPick: (id: string) => void;
};

/** The menu itself, anchored to an invisible point at `at`. */
export function ContextMenu({ at, onClose }: { at: MenuAt | null; onClose: () => void }) {
  if (!at) return null;
  return (
    <Dropdown isOpen onOpenChange={(open) => !open && onClose()}>
      <Button
        aria-hidden
        className="pointer-events-none fixed size-px min-w-0 p-0 opacity-0"
        style={{ left: at.x, top: at.y }}
      />
      <Dropdown.Popover placement="bottom start" className="min-w-52">
        <Dropdown.Menu
          aria-label={at.title}
          autoFocus="first"
          onAction={(k) => {
            const id = String(k);
            if (at.items.find((e) => e.id === id)?.now) at.onPick(id);
            onClose();
            // Once the menu is gone and the focus is back where it was: a
            // dialog the pick opens then returns the focus there too.
            if (!at.items.find((e) => e.id === id)?.now) window.setTimeout(() => at.onPick(id));
          }}
        >
          {groups(at).map((g, n) => (
            <Fragment key={n}>
              {n > 0 ? <Separator /> : null}
              <Dropdown.Section>
                <Header className="max-w-64 truncate">{g.title}</Header>
                {g.items.map((e) => (
                  <Dropdown.Item key={e.id} id={e.id} textValue={e.label} variant={e.danger ? "danger" : "default"}>
                    {e.icon}
                    <Label>{e.label}</Label>
                    {e.checked ? <Check className="ml-auto size-4 text-accent" /> : null}
                  </Dropdown.Item>
                ))}
              </Dropdown.Section>
            </Fragment>
          ))}
        </Dropdown.Menu>
      </Dropdown.Popover>
    </Dropdown>
  );
}

/** The entries in their groups: the first under the menu's title. */
function groups(at: MenuAt) {
  const out: { title: string; items: MenuEntry[] }[] = [{ title: at.title, items: [] }];
  for (const e of at.items) {
    if (e.section && out[out.length - 1].items.length) out.push({ title: e.section, items: [] });
    out[out.length - 1].items.push(e);
  }
  return out;
}

/** Right click, or a long press on a touch screen (500 ms, not moving),
 *  opens a menu at that point; the tap that ends a long press is not a tap.
 *  The keyboard's menu key and Shift+F10 send a right click as well. */
export function useContextPress(open: (x: number, y: number) => void) {
  const timer = useRef<number | undefined>(undefined);
  const start = useRef<{ x: number; y: number } | null>(null);
  const fired = useRef(false);
  const cancel = () => {
    clearTimeout(timer.current);
    start.current = null;
    // The click a long press may end in comes at once; a later one (iOS
    // sends none after a long press) is a real tap.
    if (fired.current) window.setTimeout(() => (fired.current = false), 400);
  };
  return {
    onContextMenu: (e: React.MouseEvent) => {
      e.preventDefault();
      if (fired.current) return; // the long press opened it already
      open(e.clientX, e.clientY);
    },
    onTouchStart: (e: React.TouchEvent) => {
      fired.current = false;
      if (e.touches.length !== 1) return cancel();
      const { clientX: x, clientY: y } = e.touches[0];
      start.current = { x, y };
      clearTimeout(timer.current);
      timer.current = window.setTimeout(() => {
        fired.current = true;
        start.current = null;
        navigator.vibrate?.(15);
        open(x, y);
      }, 500);
    },
    onTouchMove: (e: React.TouchEvent) => {
      const s = start.current;
      if (s && Math.hypot(e.touches[0].clientX - s.x, e.touches[0].clientY - s.y) > 10) cancel();
    },
    onTouchEnd: cancel,
    onTouchCancel: cancel,
    onClickCapture: (e: React.MouseEvent) => {
      if (fired.current) {
        fired.current = false;
        e.stopPropagation();
        e.preventDefault();
      }
    },
  };
}
