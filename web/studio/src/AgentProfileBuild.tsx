import { useState, type FormEvent } from "react";

import {
  createAgentProfileRevisionDraft,
  publishAgentProfile,
  type AgentProfileCatalog,
  type AgentProfilePublishOutcome,
  type AgentProfilePublishRequest,
  type AgentProfileRevision,
} from "./agentProfiles";

export interface AgentProfileBuildProps {
  catalog: AgentProfileCatalog | null;
  onPublished?: (profile: AgentProfileRevision) => void;
}

export function AgentProfileBuild({ catalog, onPublished }: AgentProfileBuildProps) {
  const [draft, setDraft] = useState<AgentProfilePublishRequest>(() =>
    createAgentProfileRevisionDraft());
  const [configuration, setConfiguration] = useState("{}");
  const [outcome, setOutcome] = useState<AgentProfilePublishOutcome | null>(null);
  const [publishing, setPublishing] = useState(false);

  if (catalog === null) {
    return (
      <section aria-labelledby="agent-profile-build-title">
        <p className="eyebrow">Build · External Agent configuration</p>
        <h2 id="agent-profile-build-title">Agent Profiles</h2>
        <div role="status">
          <strong>Agent Profile catalog unavailable</strong>
          <p>Publishing is disabled until the protected Supervisor store is available.</p>
        </div>
      </section>
    );
  }

  function beginRevision(profile: AgentProfileRevision) {
    const next = createAgentProfileRevisionDraft(profile);
    setDraft(next);
    setConfiguration(JSON.stringify(next.non_secret_configuration, null, 2));
    setOutcome(null);
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    let parsed: unknown;
    try {
      parsed = JSON.parse(configuration);
    } catch {
      setOutcome({ kind: "rejected" });
      return;
    }
    if (!isStringRecord(parsed)) {
      setOutcome({ kind: "rejected" });
      return;
    }
    setPublishing(true);
    const result = await publishAgentProfile({ ...draft, non_secret_configuration: parsed });
    setPublishing(false);
    setOutcome(result);
    if (result.kind === "published") onPublished?.(result.profile);
  }

  return (
    <section aria-labelledby="agent-profile-build-title">
      <div>
        <p className="eyebrow">Build · External Agent configuration</p>
        <h2 id="agent-profile-build-title">Agent Profiles</h2>
        <p>Publish non-secret policy and exact kind-bound secret settings as an immutable revision.</p>
      </div>

      {catalog.profiles.length === 0 ? (
        <p>No Agent Profile revisions have been published.</p>
      ) : (
        <ul aria-label="Published Agent Profile revisions">
          {catalog.profiles.map((profile) => (
            <li key={`${profile.profile_id}\0${profile.revision}`}>
              <strong>{profile.display_name}</strong>
              <span>{profile.profile_id} · Revision {profile.revision}</span>
              <span>{profile.secret_settings.every((setting) => setting.availability === "configured")
                ? "Provider settings configured"
                : "Provider settings need attention"}</span>
              <button type="button" onClick={() => beginRevision(profile)}>Start new revision</button>
            </li>
          ))}
        </ul>
      )}

      <form onSubmit={(event) => void submit(event)}>
        <label>
          Profile ID
          <input
            value={draft.profile_id}
            onChange={(event) => setDraft({ ...draft, profile_id: event.currentTarget.value })}
          />
        </label>
        <label>
          Revision
          <input
            value={draft.revision}
            onChange={(event) => setDraft({ ...draft, revision: event.currentTarget.value })}
          />
        </label>
        <label>
          Display name
          <input
            value={draft.display_name}
            onChange={(event) => setDraft({ ...draft, display_name: event.currentTarget.value })}
          />
        </label>
        <label>
          Non-secret configuration (JSON object)
          <textarea value={configuration} onChange={(event) => setConfiguration(event.currentTarget.value)} />
        </label>
        {draft.secret_settings.map((setting, index) => (
          <fieldset key={index}>
            <legend>Model provider setting {index + 1}</legend>
            <label>
              Model provider setting key
              <input
                value={setting.key}
                onChange={(event) => setDraft({
                  ...draft,
                  secret_settings: draft.secret_settings.map((candidate, candidateIndex) =>
                    candidateIndex === index
                      ? { ...candidate, key: event.currentTarget.value }
                      : candidate),
                })}
              />
            </label>
            <label>
              Model provider secret reference
              <input
                type="password"
                autoComplete="off"
                value={setting.reference}
                onChange={(event) => setDraft({
                  ...draft,
                  secret_settings: draft.secret_settings.map((candidate, candidateIndex) =>
                    candidateIndex === index
                      ? { ...candidate, reference: event.currentTarget.value }
                      : candidate),
                })}
              />
            </label>
          </fieldset>
        ))}
        <button
          type="button"
          onClick={() => setDraft({
            ...draft,
            secret_settings: [...draft.secret_settings, {
              key: "MODEL_PROVIDER_TOKEN",
              kind: "model_provider",
              reference: "",
            }],
          })}
        >
          Add model provider setting
        </button>
        <p>The opaque reference is used only for publication and is not returned to the browser.</p>
        <button type="submit" disabled={publishing}>
          {publishing ? "Publishing…" : "Publish immutable revision"}
        </button>
      </form>

      <PublishOutcome outcome={outcome} />
    </section>
  );
}

function PublishOutcome({ outcome }: { outcome: AgentProfilePublishOutcome | null }) {
  if (outcome === null) return null;
  if (outcome.kind === "published") {
    return <p role="status">Published exact revision {outcome.profile.revision}.</p>;
  }
  if (outcome.kind === "revision_conflict") {
    return <p role="alert">That revision is immutable. Choose a new revision identity.</p>;
  }
  if (outcome.kind === "rejected") {
    return <p role="alert">Profile settings were rejected. Remove credential material from non-secret configuration.</p>;
  }
  return <p role="alert">Profile publication is unavailable. Verify Supervisor storage and retry.</p>;
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    && Object.values(value).every((setting) => typeof setting === "string");
}
