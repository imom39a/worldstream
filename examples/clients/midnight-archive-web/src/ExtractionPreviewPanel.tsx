import type { MidnightArchiveActionType } from "./actionContract";
import type { MidnightArchiveActionIntent, MidnightArchiveProjection } from "./model";

export function ExtractionPreviewPanel({ projection, enabled, offerTypes, onAction }: {
  readonly projection: MidnightArchiveProjection;
  readonly enabled: boolean;
  readonly offerTypes: ReadonlySet<MidnightArchiveActionType>;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  if (projection.stagedAction?.actionType !== "stage_extract" || projection.phase !== "active") return null;
  const preview = projection.extraction;
  if (preview.status === "none") {
    const available = enabled && offerTypes.has("prepare_extraction");
    return <section className="extraction-preview" aria-labelledby="extraction-preview-title">
      <p className="archive-kicker">Extraction check</p><h2 id="extraction-preview-title">Preview the returning crew</h2>
      <p>Prepare a post-resolution preview before acknowledging and committing extraction.</p>
      <button type="button" data-action-type="prepare_extraction" disabled={!available}
        onClick={() => onAction({ action: "prepare_extraction" })}>Prepare extraction preview</button>
    </section>;
  }
  const left = preview.leftBehindRoles;
  const available = preview.status === "prepared" && enabled && offerTypes.has("acknowledge_extraction");
  return <section className={`extraction-preview extraction-${preview.status}`} aria-labelledby="extraction-preview-title">
    <p className="archive-kicker">Extraction check · preview {preview.revision}</p>
    <h2 id="extraction-preview-title">Post-resolution crew</h2>
    <dl>
      <div><dt>Extracted</dt><dd>{roleList(preview.extractedRoles)}</dd></div>
      <div><dt>Left behind</dt><dd>{left.length === 0 ? "No one" : roleList(left)}</dd></div>
    </dl>
    <p>{left.length === 0 ? "The entire starting crew will extract."
      : `${roleList(left)} ${left.length === 1 ? "will be" : "will be"} left behind.`}</p>
    {preview.status === "acknowledged" ? <p role="status"><strong>Exact crew result acknowledged.</strong></p> : <button
      type="button" data-action-type="acknowledge_extraction" disabled={!available}
      onClick={() => onAction({ action: "acknowledge_extraction", preview_revision: preview.revision, left_behind_roles: left })}>
      {left.length === 0 ? "Acknowledge full crew extraction" : `Acknowledge leaving ${roleList(left)} behind`}
    </button>}
  </section>;
}

function roleList(roles: readonly ("lead" | "mira" | "jonah")[]): string {
  return roles.map((role) => role === "lead" ? "Lead" : role === "mira" ? "Mira" : "Jonah").join(", ");
}
