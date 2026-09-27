import type { CSSProperties, ReactNode } from "react";

export type HeistArtworkKind = "route" | "entry_window" | "required_tool" | "extraction";
export type HeistArtworkValue = string;
export type HeistArtworkProps = { kind: HeistArtworkKind; value: HeistArtworkValue; className?: string; style?: CSSProperties; label?: ReactNode };

export const HEIST_ARTWORK_LABELS: Record<HeistArtworkKind, Record<string, string>> = {
  route: { canal: "Canal", service: "Service entrance", service_entrance: "Service entrance", roof: "Rooftop", rooftop: "Rooftop" },
  entry_window: { early: "Early window", middle: "Middle window", late: "Late window" },
  required_tool: { disguise: "Disguise", thermal_key: "Thermal key", jammer: "Jammer" },
  extraction: { van: "Van", boat: "Boat", motorbike: "Motorbike" },
};
const files: Record<HeistArtworkKind, Record<string, string>> = {
  route: { canal: "canal", service: "service-entrance", service_entrance: "service-entrance", roof: "rooftop", rooftop: "rooftop" },
  entry_window: { early: "entry-early", middle: "entry-middle", late: "entry-late" },
  required_tool: { disguise: "disguise", thermal_key: "thermal-key", jammer: "jammer" },
  extraction: { van: "van", boat: "boat", motorbike: "motorbike" },
};
const artwork = import.meta.glob<string>("./prototype-player/art/*.webp", { eager: true, query: "?url", import: "default" });
const slug = (value: string) => value.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_");
const kindLabel: Record<HeistArtworkKind, string> = { route: "Route", entry_window: "Entry window", required_tool: "Required tool", extraction: "Extraction" };

/** Presentation-only artwork. Unknown values stay visibly unknown and never infer private game data. */
export function HeistArtwork({ kind, value, className = "", style, label }: HeistArtworkProps) {
  const key = slug(value);
  const file = files[kind][key];
  const url = file ? artwork[`./prototype-player/art/${file}.webp`] : undefined;
  const readableLabel = label ?? HEIST_ARTWORK_LABELS[kind][key] ?? kindLabel[kind];
  const accessibleLabel = typeof readableLabel === "string" ? readableLabel : kindLabel[kind];
  if (!url) return <span className={`px-window-art ${className}`} style={style} data-art-kind={kind}><span>{kindLabel[kind].toUpperCase()}</span><span className="px-window-segments" aria-hidden="true">{[0, 1, 2].map((segment) => <i key={segment} className={segment === (key === "early" ? 0 : key === "middle" ? 1 : key === "late" ? 2 : -1) ? "active" : ""} />)}</span><span>{readableLabel}</span></span>;
  return <span className={`px-artwork ${className}`} style={style} data-art-kind={kind} aria-label={accessibleLabel}><img className="px-card-art" src={url} alt="" decoding="async" />{kind === "entry_window" && <span className="px-entry-window-timeline" aria-label="Entry window timeline"><i className={key === "early" ? "active" : ""} /><i className={key === "middle" ? "active" : ""} /><i className={key === "late" ? "active" : ""} /></span>}</span>;
}
