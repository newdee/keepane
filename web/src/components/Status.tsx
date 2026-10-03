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
