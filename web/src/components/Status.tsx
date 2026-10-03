import { Button, Tooltip } from "@heroui/react";
import { Fullscreen, Minimize } from "lucide-react";
import { t } from "../i18n";

/** How long the computer takes to answer, with a dot: green under 100 ms,
 *  yellow under 300, red above or when it does not answer. */
export function Latency({ ms }: { ms: number | "off" | null }) {
  if (ms === null) return null;
  const tone = ms === "off" ? "bg-danger" : ms < 100 ? "bg-success" : ms < 300 ? "bg-warning" : "bg-danger";
  const text = ms === "off" ? t("offline", "离线") : `${ms} ms`;
  return (
    <span
      id="latency"
      className="flex shrink-0 items-center gap-1.5 px-1 text-xs tabular-nums text-muted"
      title={t("Time to the computer and back", "到电脑的往返时间")}
    >
      <span className={"size-1.5 rounded-full " + tone} />
      {text}
    </span>
  );
}

/** The page over the whole screen and back; nothing where the browser
 *  cannot. */
export function FullscreenButton({ can, on, toggle }: { can: boolean; on: boolean; toggle: () => void }) {
  if (!can) return null;
  const label = on ? t("Leave full screen", "退出全屏") : t("Full screen", "全屏");
  return (
    <Tooltip delay={400}>
      <Button id="fullscreen" isIconOnly variant="ghost" size="sm" aria-label={label} onPress={toggle}>
        {on ? <Minimize className="size-4" /> : <Fullscreen className="size-4" />}
      </Button>
      <Tooltip.Content>{label}</Tooltip.Content>
    </Tooltip>
  );
}
