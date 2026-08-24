import type {
  ActivityPackCatalog,
  ActivityPackDetailResponse,
  ActivityPackReference,
} from "./activityPacks";

export interface ActivityPackCatalogViewProps {
  catalog: ActivityPackCatalog | null;
  detail: ActivityPackDetailResponse | null;
  inspectedDigest: string | null;
  selection: ActivityPackReference | null;
  loading?: boolean;
  onInspect?: (digest: string) => void;
  onSelect?: (selection: ActivityPackReference) => void;
  onClearSelection?: () => void;
}

export function ActivityPackCatalogView({
  catalog,
  detail,
  inspectedDigest,
  selection,
  loading = false,
  onInspect,
  onSelect,
  onClearSelection,
}: ActivityPackCatalogViewProps) {
  const selectedRevision = catalog?.revisions.find((revision) =>
    exactReferenceEquals(revision.pack, selection),
  );
  const shownDetail =
    detail?.revision.summary.pack.digest === inspectedDigest ? detail.revision : null;

  return (
    <section className="pack-catalog" id="build" aria-labelledby="pack-catalog-title">
      <div className="pack-catalog-heading">
        <div>
          <p className="eyebrow">Build · Installed capabilities</p>
          <h2 id="pack-catalog-title">Installed Activity Packs</h2>
          <p>Browse revisions compiled into this daemon and bind new Room drafts to one exact digest.</p>
        </div>
        <span className="bounded-chip">Host-authorized</span>
      </div>

      {selection !== null && selectedRevision === undefined ? (
        <div className="pack-selection-warning" role="alert">
          <strong>Saved revision is unavailable</strong>
          <p>{selection.id} {selection.version}</p>
          <code>{shortDigest(selection.digest)}</code>
          <p>Studio will not substitute another revision with the same name or version.</p>
          <button type="button" onClick={onClearSelection}>Clear saved reference</button>
        </div>
      ) : null}

      {catalog === null ? (
        <div className="pack-catalog-unavailable" role="status">
          <strong>{loading ? "Loading Activity Pack catalog…" : "Activity Pack catalog unavailable"}</strong>
          <p>Configure retained Host authority and confirm the daemon is available, then retry.</p>
        </div>
      ) : catalog.revisions.length === 0 ? (
        <p className="status-message">No installed Activity Pack revisions were reported.</p>
      ) : (
        <div className="pack-browser">
          <div className="pack-revision-list" aria-label="Installed Activity Pack revisions">
            {catalog.revisions.map((revision) => {
              const selected = exactReferenceEquals(revision.pack, selection);
              return (
                <article
                  className={`pack-revision ${selected ? "is-selected" : ""}`}
                  key={revision.pack.digest}
                >
                  <div>
                    <strong>{revision.name}</strong>
                    <span>{revision.pack.id} {revision.pack.version}</span>
                    <code title={revision.pack.digest}>{shortDigest(revision.pack.digest)}</code>
                  </div>
                  <div className="pack-revision-status">
                    <span>
                      {revision.selectable_for_new_rooms
                        ? "Available for new Rooms"
                        : revision.runnable_for_retained_rooms
                          ? "Retained Rooms only"
                          : "Unavailable"}
                    </span>
                    <button type="button" onClick={() => onInspect?.(revision.pack.digest)}>
                      View details
                    </button>
                    {selected ? (
                      <strong>Selected for new Room drafts</strong>
                    ) : revision.selectable_for_new_rooms ? (
                      <button type="button" onClick={() => onSelect?.(revision.pack)}>
                        Select exact revision
                      </button>
                    ) : null}
                  </div>
                </article>
              );
            })}
          </div>

          <aside className="pack-detail" aria-live="polite">
            {shownDetail === null ? (
              <p>Select “View details” to inspect a revision’s declared contract.</p>
            ) : (
              <>
                <p className="eyebrow">Exact revision contract</p>
                <h3>{shownDetail.summary.name} {shownDetail.summary.pack.version}</h3>
                <code title={shownDetail.summary.pack.digest}>
                  {shortDigest(shownDetail.summary.pack.digest)}
                </code>
                <DetailList
                  label="Roles"
                  values={shownDetail.roles.map(
                    (role) => `${role.role} · ${role.minimum}–${role.maximum}`,
                  )}
                />
                <DetailList
                  label="Actions"
                  values={shownDetail.actions.map((action) => action.action_type)}
                />
                <dl className="pack-schema-facts">
                  <div>
                    <dt>Configuration schema</dt>
                    <dd>{shownDetail.configuration_schema.schema_id}</dd>
                  </div>
                  <div>
                    <dt>Lobby compatibility</dt>
                    <dd>
                      {shownDetail.lobby_compatibility?.contract ?? "No Lobby contract declared"}
                    </dd>
                  </div>
                </dl>
              </>
            )}
          </aside>
        </div>
      )}

      <p className="pack-boundary-note">
        This catalog is read-only. Studio cannot upload packs or load executable code.
      </p>
    </section>
  );
}

function DetailList({ label, values }: { label: string; values: string[] }) {
  return (
    <div className="pack-detail-list">
      <strong>{label}</strong>
      {values.length === 0 ? <span>None declared</span> : (
        <ul>{values.map((value) => <li key={value}>{value}</li>)}</ul>
      )}
    </div>
  );
}

function exactReferenceEquals(
  left: ActivityPackReference,
  right: ActivityPackReference | null,
): boolean {
  return right !== null && left.id === right.id && left.version === right.version && left.digest === right.digest;
}

function shortDigest(digest: string): string {
  return `${digest.slice(0, 20)}…`;
}
