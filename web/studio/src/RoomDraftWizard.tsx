import type { ChangeEvent } from "react";

import type {
  ActivityPackCatalog,
  ActivityPackDetailResponse,
  ActivityPackReference,
} from "./activityPacks";
import {
  buildSeatPolicy,
  validateConfiguration,
  type RoomDraft,
  type RoomDraftFieldError,
  type RoomDraftStep,
} from "./roomDrafts";

const steps: Array<{ id: RoomDraftStep; label: string }> = [
  { id: "activity", label: "Activity" },
  { id: "configuration", label: "Configuration" },
  { id: "seats", label: "Seats" },
  { id: "readiness", label: "Readiness" },
  { id: "review", label: "Review" },
];

export interface RoomDraftWizardProps {
  draft: RoomDraft;
  catalog: ActivityPackCatalog | null;
  detail: ActivityPackDetailResponse | null;
  activeStep: RoomDraftStep;
  fieldErrors?: RoomDraftFieldError[];
  saving?: boolean;
  saved?: boolean;
  onDraftChange?: (draft: RoomDraft) => void;
  onStepChange?: (step: RoomDraftStep) => void;
  onInspectPack?: (digest: string) => void;
  onSave?: (draft: RoomDraft) => void;
}

export function RoomDraftWizard({
  draft,
  catalog,
  detail,
  activeStep,
  fieldErrors = [],
  saving = false,
  saved = false,
  onDraftChange,
  onStepChange,
  onInspectPack,
  onSave,
}: RoomDraftWizardProps) {
  const exactCatalogRevision = catalog?.revisions.find((revision) =>
    exactReferenceEquals(revision.pack, draft.pack),
  );
  const exactDetail = detail?.revision.summary.pack.digest === draft.pack?.digest ? detail : null;
  const localErrors = exactDetail === null
    ? []
    : validateConfiguration(exactDetail.revision.configuration_schema.schema, draft.configuration);
  const errors = [...localErrors, ...fieldErrors].slice(0, 64);
  const activeIndex = steps.findIndex((step) => step.id === activeStep);
  const canAdvance = stepIsValid(activeStep, draft, exactCatalogRevision !== undefined, exactDetail, errors);

  const move = (offset: number) => {
    const next = steps[activeIndex + offset];
    if (next === undefined) return;
    if (offset > 0) {
      if (!canAdvance) return;
      const progressed = { ...draft, last_valid_step: activeStep };
      onDraftChange?.(progressed);
      onSave?.(progressed);
    }
    onStepChange?.(next.id);
  };

  return (
    <section className="room-draft-wizard" id="room-draft" aria-labelledby="room-draft-title">
      <div className="draft-heading">
        <div>
          <p className="eyebrow">Build · Room draft</p>
          <h2 id="room-draft-title">Plan a new Room</h2>
          <p>Draft locally through five validated steps. Creation remains outside this surface.</p>
        </div>
        <span className="bounded-chip">Owner-only draft</span>
      </div>

      <ol className="draft-steps" aria-label="Room draft steps">
        {steps.map((step, index) => (
          <li key={step.id} className={activeStep === step.id ? "is-active" : ""}>
            <button type="button" onClick={() => onStepChange?.(step.id)}>
              {index + 1}. {step.label}
            </button>
          </li>
        ))}
      </ol>

      <div className="draft-step-panel">
        {activeStep === "activity" ? (
          <ActivityStep
            draft={draft}
            catalog={catalog}
            exactAvailable={exactCatalogRevision !== undefined}
            onChange={onDraftChange}
            onInspect={onInspectPack}
          />
        ) : null}
        {activeStep === "configuration" ? (
          <ConfigurationStep draft={draft} detail={exactDetail} errors={errors} onChange={onDraftChange} />
        ) : null}
        {activeStep === "seats" ? (
          <SeatsStep draft={draft} detail={exactDetail} onChange={onDraftChange} />
        ) : null}
        {activeStep === "readiness" ? <ReadinessStep draft={draft} /> : null}
        {activeStep === "review" ? (
          <ReviewStep draft={draft} saving={saving} saved={saved} onSave={onSave} />
        ) : null}
      </div>

      <div className="draft-navigation">
        <button type="button" disabled={activeIndex <= 0} onClick={() => move(-1)}>Back</button>
        {activeIndex < steps.length - 1 ? (
          <button type="button" disabled={!canAdvance} onClick={() => move(1)}>Continue</button>
        ) : null}
      </div>
    </section>
  );
}

function ActivityStep({
  draft,
  catalog,
  exactAvailable,
  onChange,
  onInspect,
}: {
  draft: RoomDraft;
  catalog: ActivityPackCatalog | null;
  exactAvailable: boolean;
  onChange?: (draft: RoomDraft) => void;
  onInspect?: (digest: string) => void;
}) {
  return (
    <div>
      <h3>Choose an exact Activity revision</h3>
      {draft.pack !== null && !exactAvailable ? (
        <div className="draft-warning" role="alert">
          <strong>Saved exact revision is unavailable</strong>
          <p>{draft.pack.id} {draft.pack.version}</p>
          <code>{shortDigest(draft.pack.digest)}</code>
          <p>No replacement was selected. The saved draft remains inspectable.</p>
        </div>
      ) : null}
      {catalog === null ? (
        <p className="status-message">Installed Activity Pack catalog unavailable.</p>
      ) : (
        <div className="draft-pack-list">
          {catalog.revisions.map((revision) => {
            const selected = exactReferenceEquals(revision.pack, draft.pack);
            return (
              <article key={revision.pack.digest} className={selected ? "is-selected" : ""}>
                <div>
                  <strong>{revision.name}</strong>
                  <span>{revision.pack.id} {revision.pack.version}</span>
                  <code>{shortDigest(revision.pack.digest)}</code>
                </div>
                <div>
                  <button type="button" onClick={() => onInspect?.(revision.pack.digest)}>Inspect</button>
                  {!selected && revision.selectable_for_new_rooms ? (
                    <button
                      type="button"
                      onClick={() => onChange?.({
                        ...draft,
                        pack: revision.pack,
                        configuration: {},
                        seats: [],
                        readiness: [],
                        last_valid_step: null,
                      })}
                    >Use exact revision</button>
                  ) : selected ? <strong>Selected exact revision</strong> : null}
                </div>
              </article>
            );
          })}
        </div>
      )}
    </div>
  );
}

function ConfigurationStep({
  draft,
  detail,
  errors,
  onChange,
}: {
  draft: RoomDraft;
  detail: ActivityPackDetailResponse | null;
  errors: RoomDraftFieldError[];
  onChange?: (draft: RoomDraft) => void;
}) {
  if (detail === null) {
    return <div className="draft-warning"><strong>Exact revision schema unavailable</strong><p>Return to Activity and inspect the saved exact revision.</p></div>;
  }
  const schema = detail.revision.configuration_schema.schema;
  const objectSchema = isRecord(schema) && isRecord(schema.properties) ? schema : null;
  const configuration = isRecord(draft.configuration) ? draft.configuration : {};
  return (
    <div>
      <h3>Configure declared fields</h3>
      <p>Schema {detail.revision.configuration_schema.schema_id}</p>
      {objectSchema === null ? (
        <p className="draft-warning">This exact schema cannot be rendered safely.</p>
      ) : (
        <div className="draft-fields">
          {Object.entries(objectSchema.properties as Record<string, unknown>).map(([name, fieldSchema]) => (
            <SchemaField
              key={name}
              name={name}
              schema={fieldSchema}
              value={configuration[name]}
              errors={errors.filter((fieldError) => fieldError.path === `/configuration/${escapePointer(name)}`)}
              onChange={(value) => onChange?.({
                ...draft,
                configuration: { ...configuration, [name]: value },
              })}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function SchemaField({
  name,
  schema,
  value,
  errors,
  onChange,
}: {
  name: string;
  schema: unknown;
  value: unknown;
  errors: RoomDraftFieldError[];
  onChange: (value: unknown) => void;
}) {
  const field = isRecord(schema) ? schema : {};
  const description = typeof field.description === "string" ? field.description : name;
  const inputId = `draft-field-${name}`;
  return (
    <label htmlFor={inputId}>
      <span>{description}</span>
      <code>{name}</code>
      {Array.isArray(field.enum) ? (
        <select id={inputId} value={String(value ?? "")} onChange={(event) => onChange(parseScalar(event, field.type))}>
          <option value="">Choose…</option>
          {field.enum.map((option) => <option key={JSON.stringify(option)} value={String(option)}>{String(option)}</option>)}
        </select>
      ) : field.type === "boolean" ? (
        <input id={inputId} type="checkbox" checked={value === true} onChange={(event) => onChange(event.currentTarget.checked)} />
      ) : (
        <input
          id={inputId}
          type={field.type === "integer" || field.type === "number" ? "number" : "text"}
          value={typeof value === "string" || typeof value === "number" ? value : ""}
          onChange={(event) => onChange(parseScalar(event, field.type))}
        />
      )}
      {errors.map((fieldError) => (
        <span className="draft-field-error" role="alert" key={`${fieldError.path}:${fieldError.code}`}>
          {fieldError.message} <code>{fieldError.path}</code>
        </span>
      ))}
    </label>
  );
}

function SeatsStep({
  draft,
  detail,
  onChange,
}: {
  draft: RoomDraft;
  detail: ActivityPackDetailResponse | null;
  onChange?: (draft: RoomDraft) => void;
}) {
  if (detail === null) return <p className="draft-warning">Exact revision Roles unavailable.</p>;
  const policy = buildSeatPolicy(detail.revision.roles);
  const seats = draft.seats.length === 0 ? policy.seats : draft.seats;
  return (
    <div>
      <h3>Declare stable seats</h3>
      <p>Required seats come from each Role minimum; remaining declared capacity is optional.</p>
      <div className="draft-seat-list">
        {seats.map((seat) => (
          <label key={seat.seat_id}>
            <span><strong>{seat.display_name}</strong><small>{seat.role} · {seat.required ? "Required" : "Optional"}</small></span>
            <span className="draft-seat-inputs">
              <input
                aria-label={`${seat.display_name} label`}
                value={seat.display_name}
                onChange={(event) => updateSeat(draft, seats, policy, seat.seat_id, {
                  display_name: event.currentTarget.value,
                }, onChange)}
              />
              <input
                aria-label={`${seat.display_name} principal ID`}
                placeholder={seat.required ? "Required principal ULID" : "Optional principal ULID"}
                value={seat.principal_id ?? ""}
                onChange={(event) => {
                  const principalId = event.currentTarget.value;
                  updateSeat(draft, seats, policy, seat.seat_id, principalId === ""
                    ? { principal_id: undefined, principal_kind: undefined, agent_assignment: undefined }
                    : { principal_id: principalId, principal_kind: seat.principal_kind ?? "human" }, onChange);
                }}
              />
              {seat.principal_id ? (
                <select
                  aria-label={`${seat.display_name} principal kind`}
                  value={seat.principal_kind ?? "human"}
                  onChange={(event) => updateSeat(draft, seats, policy, seat.seat_id, {
                    principal_kind: event.currentTarget.value === "agent" ? "agent" : "human",
                    agent_assignment: event.currentTarget.value === "agent"
                      ? seat.agent_assignment ?? "external"
                      : undefined,
                  }, onChange)}
                >
                  <option value="human">Human</option>
                  <option value="agent">Agent</option>
                </select>
              ) : null}
              {seat.principal_kind === "agent" ? (
                <select
                  aria-label={`${seat.display_name} agent assignment`}
                  value={seat.agent_assignment ?? "external"}
                  onChange={(event) => updateSeat(draft, seats, policy, seat.seat_id, {
                    agent_assignment: event.currentTarget.value === "managed" ? "managed" : "external",
                  }, onChange)}
                >
                  <option value="external">External agent</option>
                  <option value="managed">Managed agent</option>
                </select>
              ) : null}
            </span>
          </label>
        ))}
      </div>
      {draft.seats.length === 0 ? (
        <button type="button" onClick={() => onChange?.({ ...draft, ...policy })}>Use declared seat policy</button>
      ) : null}
    </div>
  );
}

function ReadinessStep({ draft }: { draft: RoomDraft }) {
  return (
    <div>
      <h3>Review seat readiness policy</h3>
      <p>This is pre-Room policy. It does not claim live participant presence.</p>
      <ul className="draft-readiness-list">
        {draft.readiness.map((seat) => (
          <li key={seat.seat_id}>
            <strong>{seat.role}</strong>
            <span>{seat.required
              ? `Required before later Room creation · ${draft.seats.find((candidate) => candidate.seat_id === seat.seat_id)?.principal_id ?? "unassigned"}`
              : "Optional seat may remain unfilled"}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ReviewStep({
  draft,
  saving,
  saved,
  onSave,
}: {
  draft: RoomDraft;
  saving: boolean;
  saved: boolean;
  onSave?: (draft: RoomDraft) => void;
}) {
  const snapshot = roomDraftReviewSnapshot(draft);
  return (
    <div>
      <h3>Review draft snapshot</h3>
      <p>This immutable view saves planning state only; it does not create a Room or authority.</p>
      <dl className="draft-review">
        <div><dt>Activity</dt><dd>{snapshot.pack ? `${snapshot.pack.id} ${snapshot.pack.version}` : "Not selected"}</dd></div>
        <div><dt>Exact digest</dt><dd><code>{snapshot.pack ? shortDigest(snapshot.pack.digest) : "Unavailable"}</code></dd></div>
        <div><dt>Seats</dt><dd>{snapshot.seats.length}</dd></div>
      </dl>
      <pre>{JSON.stringify(snapshot.configuration, null, 2)}</pre>
      <button type="button" disabled={saving} onClick={() => onSave?.({ ...draft, last_valid_step: "review" })}>
        {saving ? "Saving…" : saved ? "Draft saved" : "Save draft"}
      </button>
    </div>
  );
}

export function resumeRoomDraftStep(draft: RoomDraft): RoomDraftStep {
  if (draft.last_valid_step === null) return "activity";
  const index = steps.findIndex((step) => step.id === draft.last_valid_step);
  return steps[Math.min(index + 1, steps.length - 1)]?.id ?? "activity";
}

export function roomDraftReviewSnapshot(draft: RoomDraft) {
  return {
    pack: draft.pack === null ? null : { ...draft.pack },
    configuration: JSON.parse(JSON.stringify(draft.configuration)) as unknown,
    seats: draft.seats.map((seat) => ({ ...seat })),
    readiness: draft.readiness.map((seat) => ({ ...seat })),
  };
}

function stepIsValid(
  step: RoomDraftStep,
  draft: RoomDraft,
  exactAvailable: boolean,
  detail: ActivityPackDetailResponse | null,
  errors: RoomDraftFieldError[],
): boolean {
  if (step === "activity") return draft.pack !== null && exactAvailable;
  if (step === "configuration") return detail !== null && errors.length === 0;
  if (step === "seats") {
    if (detail === null) return false;
    const policy = buildSeatPolicy(detail.revision.roles);
    return sameSeatPolicy(draft, policy);
  }
  if (step === "readiness") return draft.readiness.length === draft.seats.length &&
    draft.readiness.every((readiness) => {
      const seat = draft.seats.find((candidate) => candidate.seat_id === readiness.seat_id);
      return seat !== undefined && seat.role === readiness.role && seat.required === readiness.required &&
        (!seat.required || (seat.principal_id !== undefined && seat.principal_kind !== undefined));
    });
  return true;
}

function sameSeatPolicy(draft: RoomDraft, policy: ReturnType<typeof buildSeatPolicy>): boolean {
  return draft.seats.length === policy.seats.length && draft.seats.every((seat, index) => {
    const declared = policy.seats[index];
    return declared !== undefined && seat.seat_id === declared.seat_id &&
      seat.role === declared.role && seat.required === declared.required && seat.display_name.length > 0;
  });
}

function exactReferenceEquals(left: ActivityPackReference, right: ActivityPackReference | null): boolean {
  return right !== null && left.id === right.id && left.version === right.version && left.digest === right.digest;
}

function shortDigest(digest: string): string {
  return `${digest.slice(0, 20)}…`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function escapePointer(value: string): string {
  return value.replaceAll("~", "~0").replaceAll("/", "~1");
}

function parseScalar(event: ChangeEvent<HTMLInputElement | HTMLSelectElement>, type: unknown): unknown {
  if (type === "integer" || type === "number") {
    const value = Number(event.currentTarget.value);
    return Number.isFinite(value) ? value : event.currentTarget.value;
  }
  return event.currentTarget.value;
}

function updateSeat(
  draft: RoomDraft,
  seats: RoomDraft["seats"],
  policy: ReturnType<typeof buildSeatPolicy>,
  seatId: string,
  patch: Partial<RoomDraft["seats"][number]>,
  onChange?: (draft: RoomDraft) => void,
) {
  const nextSeats = seats.map((seat) => seat.seat_id === seatId ? { ...seat, ...patch } : seat);
  onChange?.({ ...draft, seats: nextSeats, readiness: policy.readiness });
}
