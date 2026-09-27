import type { MidnightArchiveActionType } from "./actionContract";
import { JonahCrewCard } from "./JonahCrewCard";
import { MiraCrewCard } from "./MiraCrewCard";
import type { MidnightArchiveActionIntent, MidnightArchiveProjection } from "./model";

export function CrewPanel({ projection, enabled, offerTypes, onAction }: {
  readonly projection: MidnightArchiveProjection;
  readonly enabled: boolean;
  readonly offerTypes: ReadonlySet<MidnightArchiveActionType>;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  if (projection.mira.presence === "absent" && projection.jonah.presence === "absent") return null;
  return <div className="crew-panel" data-testid="crew-panel">
    <MiraCrewCard projection={projection} enabled={enabled} offerTypes={offerTypes} onAction={onAction} />
    <JonahCrewCard projection={projection} enabled={enabled} offerTypes={offerTypes} onAction={onAction} />
  </div>;
}
