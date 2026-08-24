import { useState, type FormEvent } from "react";

import type { RoomDraft } from "./roomDrafts";
import {
  instantiateTaskTemplate,
  publishTaskTemplate,
  type TaskTemplateCatalog,
  type TaskTemplatePublishRequest,
  type TaskTemplateRevisionView,
} from "./taskTemplates";

export interface TaskTemplateBuildProps {
  catalog: TaskTemplateCatalog | null;
  reviewedDraft: RoomDraft | null;
  onCatalogChanged?: () => void;
  onDraftCreated?: (draft: RoomDraft) => void;
}

export function TaskTemplateBuild({
  catalog,
  reviewedDraft,
  onCatalogChanged,
  onDraftCreated,
}: TaskTemplateBuildProps) {
  const [publish, setPublish] = useState<Omit<TaskTemplatePublishRequest, "source_draft_id">>({
    template_id: "",
    revision: "",
    display_name: "",
  });
  const [draftId, setDraftId] = useState("new-task-from-template");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  if (catalog === null) {
    return (
      <section aria-labelledby="task-template-build-title">
        <p className="eyebrow">Build · Reusable planning</p>
        <h2 id="task-template-build-title">Task Templates</h2>
        <div role="status">
          <strong>Task Template catalog unavailable</strong>
          <p>Publishing and reuse stay disabled until the protected Supervisor store is available.</p>
        </div>
      </section>
    );
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (reviewedDraft?.last_valid_step !== "review") return;
    setBusy(true);
    const result = await publishTaskTemplate({
      ...publish,
      source_draft_id: reviewedDraft.draft_id,
    });
    setBusy(false);
    setMessage(result === null
      ? "The immutable revision could not be published. Verify exact dependencies and choose a new revision identity."
      : `Published ${result.revision.display_name} revision ${result.revision.revision}.`);
    if (result !== null) onCatalogChanged?.();
  }

  async function instantiate(view: TaskTemplateRevisionView) {
    setBusy(true);
    const result = await instantiateTaskTemplate(
      view.revision.template_id,
      view.revision.revision,
      draftId,
    );
    setBusy(false);
    setMessage(result === null
      ? "The independent draft could not be created. Resolve exact dependencies or choose another draft identity."
      : `Created editable draft ${result.draft_id}; no Room was created.`);
    if (result !== null) {
      onDraftCreated?.(result);
      onCatalogChanged?.();
    }
  }

  return (
    <section aria-labelledby="task-template-build-title">
      <div>
        <p className="eyebrow">Build · Reusable planning</p>
        <h2 id="task-template-build-title">Task Templates</h2>
        <p>Publish immutable exact build choices, then create an independent editable draft without creating a Room.</p>
      </div>

      <form onSubmit={(event) => void submit(event)}>
        <label>
          Template ID
          <input value={publish.template_id} onChange={(event) => setPublish({
            ...publish, template_id: event.currentTarget.value,
          })} />
        </label>
        <label>
          New revision
          <input value={publish.revision} onChange={(event) => setPublish({
            ...publish, revision: event.currentTarget.value,
          })} />
        </label>
        <label>
          Display name
          <input value={publish.display_name} onChange={(event) => setPublish({
            ...publish, display_name: event.currentTarget.value,
          })} />
        </label>
        <button type="submit" disabled={busy || reviewedDraft?.last_valid_step !== "review"}>
          Publish immutable revision
        </button>
        {reviewedDraft?.last_valid_step !== "review" ? (
          <p>Complete and save Review before publishing this draft as a template.</p>
        ) : (
          <p>Source draft: {reviewedDraft.draft_id}</p>
        )}
      </form>

      <label>
        New independent draft ID
        <input value={draftId} onChange={(event) => setDraftId(event.currentTarget.value)} />
      </label>

      {catalog.revisions.length === 0 ? <p>No Task Template revisions published.</p> : (
        <ul aria-label="Task Template revisions">
          {catalog.revisions.map((view) => (
            <li key={`${view.revision.template_id}\0${view.revision.revision}`}>
              <article>
                <header>
                  <div>
                    <strong>{view.revision.display_name}</strong>
                    <span>{view.revision.template_id} · Revision {view.revision.revision}</span>
                  </div>
                  <span>{dependencyLabel(view.dependencies.status)}</span>
                </header>
                <p>{view.revision.pack.id} {view.revision.pack.version}</p>
                <dl>
                  <div><dt>Exact Pack digest</dt><dd><code>{view.revision.pack.digest}</code></dd></div>
                  <div><dt>Readiness</dt><dd>{view.revision.readiness.filter((seat) => seat.required).length} required · {view.revision.readiness.filter((seat) => !seat.required).length} optional</dd></div>
                </dl>
                <ul aria-label={`${view.revision.display_name} exact assignments`}>
                  {view.revision.seats.map((seat) => (
                    <li key={seat.seat_id}>
                      <span>{seat.display_name} · {seat.role} · {seat.required ? "Required" : "Optional"}</span>
                      {seat.agent_profile === undefined ? null : (
                        <span>{seat.agent_profile.profile_id} · {seat.agent_profile.revision}</span>
                      )}
                      {seat.runner_template === undefined ? null : (
                        <span>{seat.runner_template.template_id} · {seat.runner_template.revision}</span>
                      )}
                    </li>
                  ))}
                </ul>
                {view.dependencies.issues.length === 0 ? null : (
                  <ul aria-label={`${view.revision.display_name} dependency blockers`}>
                    {view.dependencies.issues.map((issue) => (
                      <li key={`${issue.path}\0${issue.code}`}>
                        <strong>{issue.message}</strong><code>{issue.path}</code>
                      </li>
                    ))}
                  </ul>
                )}
                <p>Used by: {view.used_by_draft_ids.length === 0
                  ? "No drafts yet"
                  : view.used_by_draft_ids.join(", ")}</p>
                <button
                  type="button"
                  disabled={busy || view.dependencies.status !== "ready"}
                  onClick={() => void instantiate(view)}
                >
                  Create editable draft
                </button>
              </article>
            </li>
          ))}
        </ul>
      )}
      {message === null ? null : <p role="status">{message}</p>}
    </section>
  );
}

function dependencyLabel(status: TaskTemplateRevisionView["dependencies"]["status"]): string {
  switch (status) {
    case "ready": return "Ready";
    case "missing": return "Missing exact dependency";
    case "incompatible": return "Incompatible exact dependency";
    case "unavailable": return "Dependency check unavailable";
  }
}
